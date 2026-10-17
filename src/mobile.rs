//! Mobile scene client with annotations and calibrated pose publication. Uses the same authenticated Component consumer as
//! spatial-runner; no Portal backend requests or application secrets.
use crate::{
    model::clock_ref,
    network::{HomeConnection, subscribe_scene},
};
use anyhow::{Result, bail, ensure};
use auki_component_protocol::RemoteObservationEvent;
use auki_sdk::{
    AukiPeer, AukiPeerBootstrap, AukiPeerConfig, AuthClient, AuthEnvironment, Credentials,
    DomainSelection,
};
use serde::Serialize;
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

// AuthError's Display contract excludes credentials, tokens and response bodies.
// Do not print source chains: identity/transport sources may contain local data.
fn startup_error(error: &auki_sdk::AukiPeerBootstrapError) -> String {
    use auki_sdk::AukiPeerBootstrapError::*;
    match error {
        ConfigureAuthentication(e) => format!("Phone auth configuration failed: {e}"),
        Authenticate(e) => format!("Phone sign-in failed: {e}"),
        ListDomains(e) => format!("Phone Domain lookup failed: {e}"),
        AuthorizePeer(e) => format!("Phone Domain authorization failed: {e}"),
        LoadIdentity(_) => {
            "Phone identity could not be loaded or saved in app-private storage".into()
        }
        StartPeer(auki_sdk::AukiPeerStartError::Transport(e)) => {
            format!("Phone transport startup failed: {}", transport_error(e))
        }
        StartPeer(e) => format!("Phone P2P startup failed: {e}"),
    }
}

fn transport_error(error: &auki_p2p::Error) -> String {
    use auki_p2p::Error;
    match error {
        // These strings originate in local libp2p transport construction, not
        // authentication responses. Bound their size and strip control chars.
        Error::TransportBuild(reason) | Error::Listen { reason, .. } => {
            let stage = if matches!(error, Error::TransportBuild(_)) {
                "transport construction"
            } else {
                "listener binding"
            };
            let reason: String = reason
                .chars()
                .filter(|c| !c.is_control())
                .take(512)
                .collect();
            format!("{stage}: {reason}")
        }
        Error::Io(error) => format!(
            "local I/O: {:?} (OS code {:?})",
            error.kind(),
            error.raw_os_error()
        ),
        Error::RuntimeUnavailable => "Tokio runtime unavailable".into(),
        Error::SwarmStopped => "transport worker stopped".into(),
        // A discriminant identifies other variants without printing any payload,
        // in particular token, key, identity or remote response material.
        other => format!(
            "SDK transport error {:?} (details withheld)",
            std::mem::discriminant(other)
        ),
    }
}

#[cfg(test)]
mod startup_error_tests {
    use super::*;
    use auki_sdk::{AukiPeerBootstrapError as Error, AukiPeerStartError, AuthError};

    #[test]
    fn distinguishes_denial_network_and_local_startup() {
        assert_eq!(
            startup_error(&Error::AuthorizePeer(AuthError::DomainNotAccessible)),
            "Phone Domain authorization failed: selected Domain is not accessible"
        );
        assert_eq!(
            startup_error(&Error::AuthorizePeer(AuthError::HttpStatus {
                endpoint: "peer authorization",
                status: 503,
            })),
            "Phone Domain authorization failed: peer authorization returned HTTP 503"
        );
        assert_eq!(
            startup_error(&Error::StartPeer(
                AukiPeerStartError::ListenerStartupTimeout
            )),
            "Phone P2P startup failed: the authenticated P2P listeners did not bind within the startup deadline"
        );
    }

    #[test]
    fn transport_diagnostic_is_specific_and_omits_sensitive_payloads() {
        let error = Error::StartPeer(AukiPeerStartError::Transport(
            auki_p2p::Error::TransportBuild("failed to read /etc/resolv.conf\n".into()),
        ));
        assert_eq!(
            startup_error(&error),
            "Phone transport startup failed: transport construction: failed to read /etc/resolv.conf"
        );
        assert!(
            !transport_error(&auki_p2p::Error::InvalidToken("secret-token".into()))
                .contains("secret-token")
        );
        assert!(
            !transport_error(&auki_p2p::Error::Io(std::io::Error::other("private-path")))
                .contains("private-path")
        );
        assert!(transport_error(&auki_p2p::Error::TransportBuild("x".repeat(4096))).len() < 550);
    }
}

/// How the phone signs in to Auki: a person's User account, or an app key and
/// secret (the same kind Home uses), entered on the phone and never bundled.
pub enum SignIn {
    User { email: String, password: String },
    App { key: String, secret: String },
}
impl SignIn {
    fn credentials(self) -> Result<Credentials> {
        Ok(match self {
            Self::User { email, password } => {
                ensure!(
                    !email.trim().is_empty() && !password.is_empty(),
                    "Enter your Auki User email and password"
                );
                Credentials::user_password(email, password)
            }
            Self::App { key, secret } => {
                ensure!(
                    !key.trim().is_empty() && !secret.is_empty(),
                    "Enter the app key and secret"
                );
                Credentials::app(key.trim(), secret)
            }
        })
    }
    fn failure(&self) -> &'static str {
        match self {
            Self::User { .. } => "Auki User sign-in failed. Check your account and network.",
            Self::App { .. } => "App key sign-in failed. Check the key, secret and network.",
        }
    }
}

#[derive(Clone, Serialize)]
pub struct MobileState {
    pub local_detail: String,
    pub depth_detail: String,
    #[serde(skip)]
    reference: Option<serde_json::Value>,
    #[serde(skip)]
    geometry: Option<serde_json::Value>,
    pub annotation: Option<serde_json::Value>,
    /// Home's answer to the last route asked for: its `id` with a `route`, `cleared` or an `error`.
    pub route: Option<serde_json::Value>,
    /// Home's answers to the last few path changes: `id`, `done` and, when not done, `error`.
    /// A list, so an answer is not lost to the next status before the app reads it.
    pub paths: std::collections::VecDeque<serde_json::Value>,
    pub version: u64,
    pub state: &'static str,
    pub detail: String,

    /// Validated Portal snapshot plus renderable scene geometry, in Home metres.
    pub portals: Option<serde_json::Value>,
    /// Onboarding this phone hosts or takes part in. Filled in by `snapshot`.
    pub onboarding: Option<serde_json::Value>,
}

/// Fixed port the phone listens on when it knows its Wi-Fi address, so Home
/// and robots can dial it directly instead of only through Auki's relay.
pub const DIRECT_PORT: u16 = 4330;

pub struct MobileClient {
    timing: Mutex<Option<Arc<crate::mobile_products::CaptureTiming>>>,
    capture_clock_reset: CancellationToken,
    camera: tokio::sync::watch::Sender<Option<(u64, Vec<u8>, Instant, Option<crate::mobile_camera::Geometry>)>>,
    camera_configuration:tokio::sync::watch::Sender<Option<crate::mobile_camera::RegisteredConfiguration>>,
    camera_active: AtomicBool,
    camera_demand: AtomicBool,
    /// The app's screen: the size the app sends it at, its latest picture,
    /// and whether anyone is subscribed to it.
    screen_size: tokio::sync::watch::Sender<Option<(u32, u32)>>,
    screen: tokio::sync::watch::Sender<Option<(u64, Vec<u8>, Instant)>>,
    screen_demand: AtomicBool,
    local_map: tokio::sync::watch::Sender<Option<(crate::mobile_local::Snapshot, Instant)>>,
    depth: tokio::sync::watch::Sender<Option<(crate::mobile_depth::Sample, std::time::Instant)>>,
    /// Where the AR session sits in the scene. None while the phone does not know.
    placement: tokio::sync::watch::Sender<Option<crate::mobile_declared::Placement>>,
    pose: tokio::sync::watch::Sender<Option<crate::mobile_pose::Sample>>,
    /// Every pose in order, for this phone's own pose Product. The watch above
    /// keeps only the latest one, which is right for Home's presence push but
    /// would drop the pose that belongs to a camera frame.
    pose_log: tokio::sync::mpsc::Sender<Option<crate::mobile_pose::Sample>>,
    pose_log_receiver: Mutex<Option<tokio::sync::mpsc::Receiver<Option<crate::mobile_pose::Sample>>>>,
    depth_image: tokio::sync::watch::Sender<Option<(crate::mobile_depth_image::DepthImage, Instant)>>,
    asleep: AtomicBool,
    /// The name this phone asks Home to show for it. Empty means unset.
    name: tokio::sync::watch::Sender<String>,
    /// Where this phone places the scene on the Earth. Sent on connect and when it improves.
    geo: tokio::sync::watch::Sender<Option<crate::mobile_geo::Reference>>,
    /// Latest joystick input: (robot peer, forward, turn). None means released.
    drive: tokio::sync::watch::Sender<Option<(String, f32, f32)>>,
    stop: CancellationToken,
    state: Mutex<MobileState>,
    finished: AtomicBool,
    commands: tokio::sync::mpsc::Sender<crate::mobile_notes::PlaceNote>,
    changes: tokio::sync::mpsc::Sender<crate::mobile_notes::ChangeNote>,
    change_receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::mobile_notes::ChangeNote>>>,
    portal_reports: tokio::sync::mpsc::Sender<crate::mobile_portal::Sighting>,
    portal_receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::mobile_portal::Sighting>>>,
    path_changes: tokio::sync::mpsc::Sender<crate::scene_paths::Change>,
    path_receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::scene_paths::Change>>>,
    route_asks: tokio::sync::mpsc::Sender<crate::scene_navigation::Asked>,
    route_receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::scene_navigation::Asked>>>,
    origin_reports: tokio::sync::mpsc::Sender<crate::scene_origin::Observation>,
    origin_receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::scene_origin::Observation>>>,
    receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::mobile_notes::PlaceNote>>>,
    onboarding: crate::mobile_onboarding::Shared,
    /// Home's voxels as this phone holds them, and the chunks to redraw.
    voxels: Mutex<crate::voxel_delta::Mirror>,
    /// The connected Home and how other phones reach this one. None until connected.
    connection: Mutex<Option<(crate::network::HomeConnection, crate::onboarding::Host, Arc<crate::mobile_home::HomeOperations>)>>,
    onboarding_request: Mutex<Option<String>>,
    invitations: tokio::sync::watch::Sender<Option<crate::onboarding::Invitation>>,
    sightings: tokio::sync::mpsc::Sender<crate::mobile_onboarding::Sighting>,
    sighting_receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::mobile_onboarding::Sighting>>>,
    seen: Mutex<crate::onboarding::Seen>,
}
impl Default for MobileClient {
    fn default() -> Self {
        let (commands, receiver) = tokio::sync::mpsc::channel(1);
        let (changes, change_receiver) = tokio::sync::mpsc::channel(1);
        let (portal_reports, portal_receiver) = tokio::sync::mpsc::channel(4);
        let (path_changes, path_receiver) = tokio::sync::mpsc::channel(16);
        let (route_asks, route_receiver) = tokio::sync::mpsc::channel(4);
        let (pose_log, pose_log_receiver) = tokio::sync::mpsc::channel(256);
        let (origin_reports, origin_receiver) = tokio::sync::mpsc::channel(1);
        let (sightings, sighting_receiver) = tokio::sync::mpsc::channel(4);
        Self {
            timing: Mutex::new(None),
            capture_clock_reset: CancellationToken::new(),
            onboarding: Default::default(),
            voxels: Default::default(),
            connection: Mutex::new(None),
            onboarding_request: Mutex::new(None),
            invitations: tokio::sync::watch::channel(None).0,
            sightings,
            sighting_receiver: Mutex::new(Some(sighting_receiver)),
            seen: Default::default(),
            pose_log,
            pose_log_receiver: Mutex::new(Some(pose_log_receiver)),
            depth_image: tokio::sync::watch::channel(None).0,
            origin_reports, origin_receiver:Mutex::new(Some(origin_receiver)),
            changes,
            change_receiver: Mutex::new(Some(change_receiver)),
            portal_reports,
            portal_receiver: Mutex::new(Some(portal_receiver)),
            path_changes,
            path_receiver: Mutex::new(Some(path_receiver)),
            route_asks,
            route_receiver: Mutex::new(Some(route_receiver)),
            camera: tokio::sync::watch::channel(None).0,
            camera_configuration:tokio::sync::watch::channel(None).0,
            camera_active: AtomicBool::new(false),
            camera_demand: AtomicBool::new(false),
            screen_size: tokio::sync::watch::channel(None).0,
            screen: tokio::sync::watch::channel(None).0,
            screen_demand: AtomicBool::new(false),
            local_map: tokio::sync::watch::channel(None).0,
            pose: tokio::sync::watch::channel(None).0,
            asleep: AtomicBool::new(false),
            drive: tokio::sync::watch::channel(None).0,
            name: tokio::sync::watch::channel(String::new()).0,
            geo: tokio::sync::watch::channel(None).0,
            depth: tokio::sync::watch::channel(None).0,
            placement: tokio::sync::watch::channel(None).0,
            commands,
            receiver: Mutex::new(Some(receiver)),
            stop: CancellationToken::new(),
            state: Mutex::new(MobileState {
                version: 1,
                depth_detail: "Depth not shared yet".into(),
                local_detail: "Local map not shared yet".into(),
                reference: None,
                geometry: None,
                annotation: None,
                route: None,
                paths: Default::default(),
                state: "connecting",
                detail: "Signing in to Auki development services".into(),

                portals: None,
                onboarding: None,
            }),
            finished: AtomicBool::new(false),
        }
    }
}
impl MobileClient {
    pub fn clock_generation(&self) -> u64 {
        self.timing.lock().ok().and_then(|t| t.as_ref().map(|t| t.generation)).unwrap_or(0)
    }
    /// Both brackets must belong to the same SDK clock, even if CLIENT changes
    /// between the separate native calls. Zero means absent/replaced clock.
    pub fn session_now_ns(&self, generation: u64) -> u64 {
        self.timing.lock().ok().and_then(|t| t.as_ref().filter(|t| t.generation == generation).map(|t| t.clock.now_ns())).unwrap_or(0)
    }
    pub fn observe_native_clock(&self, generation: u64, kind: usize, before: u64, native: u64, after: u64) -> bool {
        if before == 0 || after == 0 { return false; }
        let result = self.timing.lock().ok().and_then(|t| t.as_ref().filter(|t| t.generation == generation).map(|t| t.observe(kind, before, native, after)));
        match result {
            Some(Ok(())) => true,
            Some(Err(error)) => {
                if error.downcast_ref::<crate::capture_time::NativeClockReset>().is_some() { self.capture_clock_reset.cancel(); }
                self.local_status(format!("Capture clock: {error}")); false
            }
            None => false,
        }
    }
    pub fn camera_demanded(&self, active: bool) -> bool {
        self.camera_active.store(active, Ordering::Release);
        if !active { self.camera.send_replace(None); }
        active && self.camera_demand.load(Ordering::Acquire) && !self.stop.is_cancelled()
    }
    /// Offer the app's screen at `size` and say whether anyone wants it.
    pub fn screen_demanded(&self, size: (u32, u32)) -> bool {
        if crate::mobile_screen::size_allowed(size) {
            self.screen_size.send_if_modified(|current| current.replace(size) != Some(size));
        }
        self.screen_demand.load(Ordering::Acquire) && !self.stop.is_cancelled()
    }
    pub fn update_screen(&self, timestamp: u64, jpeg: Vec<u8>) {
        if self.screen_demand.load(Ordering::Acquire) {
            self.screen.send_replace(Some((timestamp, jpeg, Instant::now())));
        }
    }
    pub fn update_camera(&self, timestamp: u64, jpeg: Vec<u8>) {
        self.send_camera(timestamp, jpeg, None);
    }
    /// A frame with the AR session that took it and its intrinsics as sent.
    pub fn update_camera_calibrated(&self, timestamp: u64, jpeg: Vec<u8>, geometry: crate::mobile_camera::Geometry) {
        self.send_camera(timestamp, jpeg, Some(geometry));
    }
    fn send_camera(&self, timestamp: u64, jpeg: Vec<u8>, geometry: Option<crate::mobile_camera::Geometry>) {
        if self.camera_active.load(Ordering::Acquire) && self.camera_demand.load(Ordering::Acquire) {
            self.camera.send_replace(Some((timestamp, jpeg, Instant::now(), geometry)));
        }
    }
    pub fn update_local_map(&self, sample: Option<crate::mobile_local::Snapshot>) {
        self.local_map
            .send_replace(sample.map(|s| (s, Instant::now())));
    }
    pub fn update_depth(&self, sample: Option<crate::mobile_depth::Sample>) {
        self.depth
            .send_replace(sample.map(|s| (s, std::time::Instant::now())));
    }
    /// The session-to-scene transform the app holds, published whether or not depth is shared.
    pub fn update_placement(&self, placement: Option<crate::mobile_declared::Placement>) {
        self.placement.send_replace(placement);
    }
    fn depth_status(&self, detail: String) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.depth_detail != detail {
            state.depth_detail = detail;
            state.version += 1;
        }
    }
    fn local_status(&self, detail: String) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.local_detail != detail {
            state.local_detail = detail;
            state.version += 1;
        }
    }
    pub fn configure_camera(&self,configuration:crate::mobile_camera::Configuration)->Result<()> {
        configuration.camera.validate()?;uuid::Uuid::parse_str(&configuration.session)?;
        self.camera_configuration.send_if_modified(|current|{
            if current.as_ref().is_some_and(|old|old.configuration.session==configuration.session&&serde_json::to_value(&old.configuration.camera).ok()==serde_json::to_value(&configuration.camera).ok()){return false;}
            *current=Some(crate::mobile_camera::RegisteredConfiguration{id:uuid::Uuid::new_v4().to_string(),configuration:configuration.clone()});true
        });Ok(())
    }
    pub fn update_pose(&self, mut sample: Option<crate::mobile_pose::Sample>) {
        if let Some(sample)=&mut sample {sample.camera_configuration_id=self.camera_configuration.borrow().as_ref().filter(|c|c.configuration.session==sample.session).map(|c|c.id.clone());}
        if let Ok(mut history) = self.onboarding.history.lock() {
            let converted = (|| -> Result<_> {
                let timing = self.timing.lock().map_err(|_| anyhow::anyhow!("Capture clock unavailable"))?;
                let timing = timing.as_ref().ok_or_else(|| anyhow::anyhow!("Capture clock unavailable"))?;
                let sample = sample.as_ref().ok_or_else(|| anyhow::anyhow!("Tracking unavailable"))?;
                Ok((timing.clock_reference()?, timing.pose(sample.clone())?))
            })();
            match converted {
                Ok((clock, sample)) => history.push(&clock, &sample.session, sample.timestamp_ns, crate::local_mapper::from_rigid(&sample.pose)),
                Err(_) => history.lose(),
            }
        }
        // Capture-time compact pose history; no legacy Home presence push.
        let _ = self.pose_log.try_send(sample.clone());
        self.pose.send_replace(sample);
    }
    /// The LiDAR depth image of a camera frame. Kept only while a reader wants frames.
    pub fn update_depth_image(&self, image: crate::mobile_depth_image::DepthImage) {
        if self.camera_active.load(Ordering::Acquire) && self.camera_demand.load(Ordering::Acquire) {
            self.depth_image.send_replace(Some((image, Instant::now())));
        }
    }
    /// The phone paused its camera. Home keeps the last pose, marked asleep.
    pub fn set_asleep(&self, asleep: bool) {
        self.asleep.store(asleep, Ordering::Release);
        if asleep {
            if let Ok(mut history) = self.onboarding.history.lock() { history.lose(); }
            let _ = self.pose_log.try_send(None);
            self.pose.send_replace(None);
        }
    }
    pub fn annotate(&self, request: crate::mobile_notes::PlaceNote) -> bool {
        if self.snapshot().state != "live" {
            return false;
        }
        self.commands.try_send(request).is_ok()
    }
    pub fn change_note(&self, request: crate::mobile_notes::ChangeNote) -> bool {
        self.snapshot().state == "live" && self.changes.try_send(request).is_ok()
    }
    /// Name this phone on Home. Sent on connect and whenever it changes.
    pub fn set_name(&self, name: String) {
        self.name.send_replace(name);
    }
    /// Tell Home where the scene is on the Earth, as this phone measured it.
    pub fn set_geo(&self, reference: crate::mobile_geo::Reference) {
        self.geo.send_replace(Some(reference));
    }
    /// Set or release the joystick. Releasing sends explicit stops.
    pub fn set_drive(&self, input: Option<(String, f32, f32)>) {
        self.drive.send_replace(input);
    }
    /// Queue a drawn path for Home to save, or a saved one for Home to remove.
    pub fn change_path(&self, change: crate::scene_paths::Change) -> bool {
        self.snapshot().state == "live" && self.path_changes.try_send(change).is_ok()
    }
    /// Queue a route for Home to plan, or this phone's route for Home to take away.
    pub fn ask_route(&self, asked: crate::scene_navigation::Asked) -> bool {
        self.snapshot().state == "live" && self.route_asks.try_send(asked).is_ok()
    }
    /// Queue a newly seen Portal for Home to add to the scene.
    pub fn report_portal(&self, report: crate::mobile_portal::Sighting) -> bool {
        self.snapshot().state == "live" && self.portal_reports.try_send(report).is_ok()
    }
    pub fn report_origin(&self, observation:crate::scene_origin::Observation)->bool {
        self.snapshot().state=="live" && self.origin_reports.try_send(observation).is_ok()
    }
    pub fn origin_reference(&self)->Result<auki_scenegraph::MapSnapshot>{
        let state=self.snapshot();ensure!(state.state=="live","Home is not connected");
        let snapshot:auki_scenegraph::MapSnapshot=serde_json::from_value(state.reference.ok_or_else(||anyhow::anyhow!("Home reference unavailable"))?)?;
        snapshot.validate()?;ensure!(snapshot.scenegraph.anchors.is_empty(),"Home already has an origin");Ok(snapshot)
    }
    pub fn stop(&self) {
        self.stop.cancel();
        self.stop_onboarding();
    }
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
    pub fn snapshot(&self) -> MobileState {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // Both only grow, so the sum moves whenever either does.
        state.version += self.onboarding.revision.load(Ordering::Acquire);
        state.onboarding = self.onboarding.snapshot();
        state
    }

    /// Reserve an attempt synchronously before dispatching network work. A
    /// cancelled/older queued worker cannot supersede a new attempt or client.
    pub fn prepare_onboarding(&self) -> String {
        let attempt = uuid::Uuid::new_v4().to_string();
        *self.onboarding_request.lock().unwrap_or_else(|e| e.into_inner()) = Some(attempt.clone());
        attempt
    }
    /// Start showing the way into this phone's scene. Needs a live connection
    /// and a place in the scene. Returns the invitation QR text.
    pub async fn host_onboarding(&self, attempt: &str, alias: &str, display: crate::onboarding::Display) -> Result<String> {
        ensure!(self.onboarding_request.lock().unwrap_or_else(|e| e.into_inner()).as_deref() == Some(attempt), "Onboarding cancelled");
        ensure!(self.snapshot().state == "live", "Connect to Home first");
        let placement = self.placement.borrow().clone();
        let history = self.onboarding.history.lock().map_err(|_| anyhow::anyhow!("Pose history unavailable"))?.session().to_owned();
        ensure!(placement.as_ref().is_some_and(|p| p.session == history), "Calibrate this phone into the scene first");
        let marker_in_camera = display.marker_in_camera()?;
        let (home, host, operations) = self.connection.lock().unwrap_or_else(|e| e.into_inner()).clone()
            .ok_or_else(|| anyhow::anyhow!("Connect to Home first"))?;
        ensure!(placement.as_ref().is_some_and(|p| !p.session.is_empty()
            && p.session_to_scene.from_frame_id == format!("phone-ar:{}", p.session)
            && p.session_to_scene.to_frame_id == home.frame), "Calibrate into the selected Home scene first");
        // Fetch only when preparing a QR, over the existing authenticated SDK
        // connection. Retain credentials only in the active invitation for its
        // QR display, never in persistent phone settings or a fallback cache.
        let wifi = tokio::select! {
            _ = self.stop.cancelled() => anyhow::bail!("Onboarding cancelled"),
            result = tokio::time::timeout(Duration::from_secs(20), crate::scene_wifi::retrieve(&operations, &home)) =>
                // No Wi-Fi from Home (not permitted, not set up, older Home, no answer)
                // is not a failure: the invitation goes without it, for a robot
                // already on this network.
                match result.map_err(|_| anyhow::anyhow!("Scene Wi-Fi request timed out")).and_then(|r| r) {
                    Ok(wifi) => wifi,
                    Err(e) => { eprintln!("[onboarding] invitation without Wi-Fi: {e:#}"); None }
                },
        };
        ensure!(self.snapshot().state == "live" && self.connection.lock().unwrap_or_else(|e| e.into_inner())
            .as_ref().is_some_and(|(_, _, current)| Arc::ptr_eq(current, &operations)), "Home connection changed. Retry onboarding");
        ensure!(self.placement.borrow().as_ref().is_some_and(|p| p.session == history
            && p.session_to_scene.from_frame_id == format!("phone-ar:{history}")
            && p.session_to_scene.to_frame_id == home.frame)
            && self.onboarding.history.lock().map_err(|_| anyhow::anyhow!("Pose history unavailable"))?.session() == history,
            "Calibration changed. Calibrate this phone again");
        let invitation = crate::onboarding::Invitation::new(alias, home.clone(), host, wifi)?;
        let text = invitation.encode()?;
        let request = self.onboarding_request.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(!self.stop.is_cancelled() && request.as_deref() == Some(attempt), "Onboarding cancelled");
        let mut hosting = self.onboarding.hosting.lock().map_err(|_| anyhow::anyhow!("Onboarding unavailable"))?;
        *hosting = Some(crate::onboarding::Hosting {
            session: invitation.session.clone(), alias: invitation.alias.clone(), invitation: text.clone(),
            marker: invitation.marker_payload(), marker_in_camera, scene_peer_id: home.peer_id, scene_frame: home.frame,
            joiner: None, stage: None, detail: "Show this code to the robot".into(),
        });
        self.onboarding.touch();
        Ok(text)
    }
    /// The voxel chunks changed since the app last asked, with their cells.
    pub fn take_voxels(&self) -> Option<serde_json::Value> {
        self.voxels.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
    /// Stop hosting or joining. A joiner keeps any place it already found.
    pub fn stop_onboarding(&self) {
        *self.onboarding_request.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if let Ok(mut hosting) = self.onboarding.hosting.lock() { *hosting = None; }
        let session = self.invitations.send_replace(None).map(|i| i.session);
        if let Some(session) = session { self.seen.lock().unwrap_or_else(|e| e.into_inner()).forget(&session); }
        if let Ok(mut joining) = self.onboarding.joining.lock() { *joining = None; }
        self.onboarding.touch();
    }
    /// A scanned invitation. Starts joining the first time a session is seen,
    /// if this phone is connected to the invitation's Home. Ok(false) when it
    /// was seen before.
    pub fn join_onboarding(&self, text: &str) -> Result<bool> {
        let invitation = crate::onboarding::Invitation::parse(text)?;
        let connected = self.connection.lock().unwrap_or_else(|e| e.into_inner()).clone();
        ensure!(self.snapshot().state == "live" && connected.as_ref().is_some_and(|(home, _, _)|
            invitation.is_for(&home.peer_id, &home.frame)),
            "Connect to the invitation's Home scene first");
        if !self.seen.lock().unwrap_or_else(|e| e.into_inner()).first(&invitation.session) {
            return Ok(false);
        }
        *self.onboarding.joining.lock().map_err(|_| anyhow::anyhow!("Onboarding unavailable"))? = Some(crate::mobile_onboarding::Joining {
            session: invitation.session.clone(), alias: invitation.alias.clone(), marker: None, looking: false,
            stage: None, detail: "Joining".into(), sightings: 0, clock: None, alignment: None,
        });
        self.set_name(invitation.alias.clone());
        self.invitations.send_replace(Some(invitation));
        self.onboarding.touch();
        Ok(true)
    }
    /// A camera frame while joining. Finds the host's marker and queues it.
    pub fn onboarding_frame(&self, luma: &[u8], frame: &crate::mobile_onboarding::Frame) -> Result<bool> {
        let marker = self.onboarding.joining.lock().ok()
            .and_then(|j| j.as_ref().filter(|j| j.looking).and_then(|j| j.marker.clone()));
        let Some(marker) = marker else { return Ok(false) };
        let timing = self.timing.lock().map_err(|_| anyhow::anyhow!("Capture clock unavailable"))?.clone()
            .ok_or_else(|| anyhow::anyhow!("Capture clock unavailable"))?;
        let Some(sighting) = crate::mobile_onboarding::sight(luma, frame, &marker, &timing)? else { return Ok(false) };
        let _ = self.sightings.try_send(sighting);
        Ok(true)
    }
    fn status(&self, state: &'static str, detail: impl Into<String>) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.version += 1;
        s.state = state;
        s.detail = detail.into();
        if state != "live" {
            s.portals = None;
        }
    }

    /// `lan` is this phone's Wi-Fi IPv4 address, if it is on Wi-Fi: see `DIRECT_PORT`.
    pub async fn run(
        self: Arc<Self>,
        profile: String,
        sign_in: SignIn,
        identity: String,
        lan: Option<std::net::Ipv4Addr>,
    ) {
        let result = self
            .connect(profile, sign_in, Path::new(&identity), lan)
            .await;
        if let Err(error) = result {
            self.status("error", error.to_string());
        } else {
            self.status("stopped", "Disconnected");
        }
        self.finished.store(true, Ordering::Release);
    }

    async fn connect(
        &self,
        profile: String,
        sign_in: SignIn,
        identity: &Path,
        lan: Option<std::net::Ipv4Addr>,
    ) -> Result<()> {
        ensure!(
            profile.len() <= 32 * 1024,
            "Home connection profile is too large"
        );
        let home: HomeConnection = serde_json::from_str(&profile)?;
        // Fixed, explicit environment: profile files cannot redirect User credentials.
        ensure!(
            home.schema == "spatial-harness.home-connection/v1",
            "Unknown Home profile"
        );
        home.validate_scene_contract()?;
        ensure!(
            home.frame == crate::model::FRAME || home.local_scene_id().is_some(),
            "Expected an OpenGL Home reference scene"
        );
        ensure!(
            !home.routes.is_empty() && home.routes.len() <= 16,
            "Home needs 1–16 routes"
        );
        let _: auki_sdk::PeerId = home.peer_id.parse()?;
        for route in &home.routes {
            let _: auki_sdk::Multiaddr = route.parse()?;
        }
        let domain = home.domain_id.parse()?;
        let failure = sign_in.failure();
        let credentials = sign_in.credentials()?;
        if self.stop.is_cancelled() {
            return Ok(());
        }
        // Home pulls this phone's pose and depth through its Catalog, so Home must
        // be able to dial the phone. Discovery publishes advertised routes, never
        // listen addresses, so on a random port the phone is reachable only through
        // the relay. On Wi-Fi, also listen on a fixed port and advertise that direct
        // route; the relay stays as the fallback.
        let config = match lan {
            Some(ip) => AukiPeerConfig::dev()
                .with_listen_addresses([format!("/ip4/0.0.0.0/tcp/{DIRECT_PORT}").parse()?])?
                .with_advertised_direct_routes([format!("/ip4/{ip}/tcp/{DIRECT_PORT}").parse()?])?,
            None => AukiPeerConfig::dev().with_listen_addresses(["/ip4/0.0.0.0/tcp/0".parse()?])?,
        };
        let auth = AukiPeerBootstrap::authenticate(
            AuthClient::new(AuthEnvironment::dev())?,
            credentials,
            config,
        );
        let bootstrap = tokio::select! {
            _ = self.stop.cancelled() => return Ok(()),
            result = tokio::time::timeout(Duration::from_secs(45), auth) =>
                result.map_err(|_| anyhow::anyhow!("Sign-in timed out"))?
                    .map_err(|_| anyhow::anyhow!(failure))?,
        };
        // Await startup and then always shut it down if Stop arrived meanwhile.
        // Advertise the mounted component endpoint and current relay routes so
        // Home can inspect this phone's catalog, not just receive pushed maps.
        let bootstrap = bootstrap.with_dds_tracker(auki_sdk::DdsTrackerMode::DiscoverAndAdvertise);
        // Dropping an in-progress SDK peer startup is not used as cancellation.
        self.status("connecting", "Authorizing phone in Home's transport Domain");
        let peer = bootstrap
            .start_persistent_peer(DomainSelection::new(domain), identity)
            .await;
        let result = match peer {
            Ok(peer) => {
                let work = async {
                let operations = Arc::new(crate::mobile_home::HomeOperations::new(&peer,&home)?);
                // Warm once before high-rate producers start, so their short
                // sample deadlines cannot repeatedly cancel cold discovery.
                tokio::select! { _ = self.stop.cancelled() => return Ok(()), _ = operations.warm() => {} }
                let products = crate::mobile_products::MobileProducts::mount(&peer)?;
                *self.timing.lock().unwrap_or_else(|e| e.into_inner()) = Some(products.timing.clone());
                let clock_work = async {
                    let mut tick = tokio::time::interval(Duration::from_millis(200));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop { tick.tick().await; products.sample_time()?; }
                    #[allow(unreachable_code)]
                    Ok::<(), anyhow::Error>(())
                };
                // How a phone being onboarded reaches this one: its own addresses on the network.
                let routes: Vec<String> = peer.listen_addresses().iter().map(ToString::to_string)
                    .filter(|r| !r.contains("/127.0.0.1/") && !r.contains("/0.0.0.0/")).take(4).collect();
                *self.connection.lock().unwrap_or_else(|e| e.into_inner()) = (!routes.is_empty()).then(|| (home.clone(),
                    crate::onboarding::Host { peer_id: peer.peer_id().to_string(), routes }, operations.clone()));
                let onboarding_display = crate::mobile_onboarding::Display::mount(&products, &self.onboarding, self.placement.subscribe())?;
                let mut invitations = self.invitations.subscribe();
                let mut sightings = self.sighting_receiver.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap();
                let join_work = async {
                    loop {
                        if invitations.changed().await.is_err() { break; }
                        let Some(invitation) = invitations.borrow_and_update().clone() else { continue };
                        let joined = tokio::select! {
                            result = tokio::time::timeout(Duration::from_secs(120),
                                crate::mobile_onboarding::join(&peer, &home, &invitation, &self.onboarding, &mut sightings, &products)) =>
                                result.unwrap_or_else(|_| Err(anyhow::anyhow!("Onboarding timed out; scan the code again"))),
                            // Stopped or replaced by another invitation.
                            _ = invitations.changed() => { invitations.mark_changed(); continue }
                        };
                        match joined {
                            // The app adopts it as its own calibration, which publishes it as usual.
                            Ok(alignment) => self.update_placement(Some(crate::mobile_declared::Placement {
                                session: alignment.portal_to_session.to_frame_id.trim_start_matches("phone-ar:").to_owned(),
                                timestamp_ns: alignment.timestamp_ns, session_to_scene: alignment.session_to_home.clone() })),
                            Err(error) => {
                                self.onboarding.join(|j| { j.looking = false; j.stage = Some(crate::onboarding::Stage::Failed); j.detail = error.to_string(); });
                                self.seen.lock().unwrap_or_else(|e| e.into_inner()).forget(&invitation.session);
                            }
                        }
                    }
                };
                // What this phone declares in its own Catalog. The camera is
                // declared at connect; pose and depth as soon as the AR session
                // is known. Frames are still captured and sent only on demand.
                // Shared by the producers below; never held across an await.
                struct Declared {
                    camera: Option<crate::mobile_camera::Camera>,
                    depth_image: Option<crate::mobile_depth_image::Publisher>,
                    pose: Option<crate::mobile_odometry::Publisher>,
                    /// The phone's alignment into the scene and its depth points, for Home to read.
                    placed: Option<crate::mobile_declared::Publisher>,
                }
                let declared = std::sync::Mutex::new(Declared {
                    camera: Some(crate::mobile_camera::Camera::new(&products)?),
                    depth_image: None,
                    pose: None,
                    placed: None,
                });
                let declared = || declared.lock().unwrap_or_else(|e| e.into_inner());
                let mut frames = self.camera.subscribe();
                let mut configurations=self.camera_configuration.subscribe();
                let mut calibration_publisher=None;
                let mut published_configuration=None::<String>;
                let camera_work = async {
                    let mut tick = tokio::time::interval(Duration::from_millis(100));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        tick.tick().await;
                        let config=configurations.borrow_and_update().clone();
                        if let Some(config)=config.filter(|c|Some(&c.id)!=published_configuration.as_ref()) {
                            if let Some(old)=calibration_publisher.take(){let old:crate::mobile_camera::CalibrationPublisher=old;let _=old.close();}
                            calibration_publisher=Some(crate::mobile_camera::CalibrationPublisher::new(&products,&config)?);
                            published_configuration=Some(config.id);
                        }
                        let active = self.camera_active.load(Ordering::Acquire);
                        // A reader of either the pictures or their depth images
                        // gets both: they are one capture.
                        let demand = active && {
                            let declared = declared();
                            declared.camera.as_ref().is_some_and(|c| c.demanded())
                                || declared.depth_image.as_ref().is_some_and(|d| d.demanded())
                        };
                        self.camera_demand.store(demand, Ordering::Release);
                        if frames.has_changed().unwrap_or(false) {
                            let latest = frames.borrow_and_update().clone();
                            if demand && let Some((timestamp, jpeg, received, geometry)) = latest
                                && received.elapsed() < Duration::from_millis(500)
                                && let Some(camera) = declared().camera.as_mut() {
                                // Bad/late frames must not tear down mapping or the peer.
                                let result = (|| {
                                    let timestamp = products.timing.capture(0, timestamp)?;
                                    match geometry {
                                        Some(mut geometry) => {
                                            geometry.alignment_timestamp_ns = products.timing.held(&geometry.session, geometry.alignment_timestamp_ns)?;
                                            camera.publish_calibrated(timestamp, jpeg, geometry)
                                        }
                                        None => camera.publish(timestamp, jpeg),
                                    }
                                })();
                                if let Err(error) = result { self.local_status(format!("Camera not shared: {error}")); }
                            }
                        }
                    }
                    #[allow(unreachable_code)]
                    Ok::<(), anyhow::Error>(())
                };
                let mut screen_sizes = self.screen_size.subscribe();
                let mut screen_frames = self.screen.subscribe();
                let mut screen = None::<crate::mobile_screen::Screen>;
                let screen_work = async {
                    let mut tick = tokio::time::interval(Duration::from_millis(50));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        tick.tick().await;
                        let size = *screen_sizes.borrow_and_update();
                        if let Some(size) = size.filter(|s| screen.as_ref().is_none_or(|old| old.size() != *s)) {
                            if let Some(old) = screen.take() { let _ = old.close(); }
                            // The screen is optional: failing to offer it must not drop the connection.
                            screen = crate::mobile_screen::Screen::new(&products, size).ok();
                        }
                        let demand = screen.as_ref().is_some_and(|s| s.demanded());
                        self.screen_demand.store(demand, Ordering::Release);
                        if screen_frames.has_changed().unwrap_or(false) {
                            let latest = screen_frames.borrow_and_update().clone();
                            if demand && let Some((timestamp, jpeg, received)) = latest
                                && received.elapsed() < Duration::from_millis(500)
                                && let Some(screen) = screen.as_mut() {
                                // A bad frame must not tear down the connection.
                                if let Err(error) = products.timing.capture(1, timestamp).and_then(|at| screen.publish(at, jpeg)) {
                                    self.local_status(format!("Screen not shared: {error}"));
                                }
                            }
                        }
                    }
                    #[allow(unreachable_code)]
                    Ok::<(), anyhow::Error>(())
                };
                let mut receiver = self
                    .receiver
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                    .unwrap();
                let commands = async {
                    while let Some(request) = receiver.recv().await {
                        let result = crate::mobile_notes::send(&peer, &home, &request).await;
                        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        state.version += 1;
                        state.annotation = Some(match result {
                            Ok(note) => {
                                serde_json::json!({"id":request.id,"saved":true,"detail":"Saved to Home","note":note})
                            }
                            Err(error) => {
                                serde_json::json!({"id":request.id,"saved":false,"detail":error.to_string()})
                            }
                        });
                    }
                };
                let mut changes = self.change_receiver.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap();
                let mut portal_reports = self.portal_receiver.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap();
                let mut origin_reports=self.origin_receiver.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap();
                let origin_work=async {
                    while let Some(observation)=origin_reports.recv().await {
                        let result=operations.invoke::<_,bool>(&home.scoped_component(crate::mobile_portal::COMPONENT),crate::scene_origin::OPERATION,"portal-observer",&uuid::Uuid::new_v4().to_string(),Duration::from_secs(3),&observation).await;
                        self.local_status(match result {Ok(true)=>format!("Portal {} added to Home as its origin",observation.code),Ok(false)=>"Home refused origin observation".into(),Err(e)=>format!("Origin not set: {e}")});
                    }
                };
                let mut path_changes = self.path_receiver.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap();
                let path_work = async {
                    use crate::scene_paths::Change;
                    while let Some(change) = path_changes.recv().await {
                        let component = home.scoped_component(crate::scene_paths::COMPONENT);
                        let request = uuid::Uuid::new_v4().to_string();
                        let (id, done, doing) = match &change {
                            Change::Add(add) => (add.stroke.id.clone(), "saved", "saved"),
                            Change::Remove(remove) => (remove.id.clone(), "removed", "removed"),
                        };
                        let result = tokio::time::timeout(Duration::from_secs(4), async {
                            match &change {
                                Change::Add(add) => operations.invoke::<_, bool>(&component, "add", "phone-path", &request, Duration::from_secs(3), add).await,
                                Change::Remove(remove) => operations.invoke::<_, bool>(&component, "remove", "phone-path", &request, Duration::from_secs(3), remove).await,
                            }
                        })
                        .await;
                        let error = match result {
                            Ok(Ok(true)) => None,
                            Ok(Ok(false)) => Some("Home refused it".to_string()),
                            Ok(Err(error)) => Some(error.to_string()),
                            Err(_) => Some("Home did not answer".to_string()),
                        };
                        self.local_status(match &error {
                            None => format!("Path {id} {done}"),
                            Some(error) => format!("Path {id} not {doing}: {error}"),
                        });
                        // The app matches these on the path's ID.
                        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        state.version += 1;
                        if state.paths.len() == 16 { state.paths.pop_front(); }
                        state.paths.push_back(serde_json::json!({"id": id, "done": error.is_none(), "error": error}));
                    }
                };
                let mut route_asks = self.route_receiver.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap();
                let route_work = async {
                    use crate::scene_navigation::{Ask, COMPONENT, Route};
                    while let Some(asked) = route_asks.recv().await {
                        let component = home.scoped_component(COMPONENT);
                        let request = uuid::Uuid::new_v4().to_string();
                        let result = tokio::time::timeout(Duration::from_secs(4), async {
                            Ok::<_, anyhow::Error>(match &asked.ask {
                                Ask::Plan(plan) => serde_json::json!({"id": asked.id, "route":
                                    operations.invoke::<_, Route>(&component, "plan", "phone-route", &request, Duration::from_secs(3), plan).await?}),
                                Ask::Clear(clear) => serde_json::json!({"id": asked.id, "cleared":
                                    operations.invoke::<_, bool>(&component, "clear", "phone-route", &request, Duration::from_secs(3), clear).await?}),
                            })
                        })
                        .await;
                        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        state.version += 1;
                        // The app matches these on the ID it asked with.
                        state.route = Some(match result {
                            Ok(Ok(answer)) => answer,
                            Ok(Err(error)) => serde_json::json!({"id": asked.id, "error": error.to_string()}),
                            Err(_) => serde_json::json!({"id": asked.id, "error": "Home did not answer"}),
                        });
                    }
                };
                let portal_work = async {
                    while let Some(report) = portal_reports.recv().await {
                        let code = report.code.clone();
                        let (done, doing) = if report.replace { ("moved in", "moved") } else { ("added to", "added") };
                        let component = home.scoped_component(crate::mobile_portal::COMPONENT);
                        let request = uuid::Uuid::new_v4().to_string();
                        let result = if let Some(alignment) = report.alignment.clone() {
                            let report = crate::mobile_portal::Report {
                                session: report.session.clone(), timestamp_ns: report.timestamp_ns,
                                code: code.clone(), side_length_m: report.side_length_m,
                                portal_to_session: report.portal_to_session.clone(), alignment,
                                replace: report.replace,
                            };
                            tokio::time::timeout(
                                Duration::from_secs(4),
                                operations.invoke::<_, bool>(&component, "add", "phone-portal", &request,
                                    Duration::from_secs(3), &report),
                            )
                            .await
                        } else {
                            // The first Portal of an empty scene: Home makes it the origin.
                            let mut scene = self.state.lock().unwrap_or_else(|e| e.into_inner()).reference.clone();
                            if let Some(render) = scene.as_mut().and_then(|s| s.as_object_mut()) { render.remove("render"); }
                            let Some(Ok(expected_scene)) = scene.map(serde_json::from_value) else {
                                self.local_status(format!("Portal {code} not added: no scene received from Home yet"));
                                continue;
                            };
                            let first = crate::scene_origin::Observation {
                                expected_scene, code: code.clone(), side_length_m: report.side_length_m,
                                timestamp_ns: report.timestamp_ns, portal_to_native: report.portal_to_session.clone(),
                            };
                            tokio::time::timeout(
                                Duration::from_secs(4),
                                operations.invoke::<_, bool>(&component, crate::scene_origin::OPERATION, "phone-portal", &request,
                                    Duration::from_secs(3), &first),
                            )
                            .await
                        };
                        self.local_status(match result {
                            Ok(Ok(true)) => format!("Portal {code} {done} Home's scene"),
                            Ok(Ok(false)) => format!("Home refused Portal {code}"),
                            Ok(Err(error)) => format!("Portal {code} not {doing}: {error}"),
                            Err(_) => format!("Portal {code} not {doing}: Home did not answer"),
                        });
                    }
                };
                let change_work = async {
                    while let Some(request) = changes.recv().await {
                        let result = tokio::time::timeout(Duration::from_secs(25), crate::mobile_notes::send_change_via(&operations, &home, &request)).await
                            .unwrap_or_else(|_| Err(anyhow::anyhow!("Home did not confirm the note change; retry to check its result")));
                        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        state.version += 1;
                        state.annotation = Some(match result {
                            Ok(change) => serde_json::json!({"id":request.request_id,"saved":true,"deleted":change.deleted,"note":change.note,"detail":"Saved to Home"}),
                            Err(e) => serde_json::json!({"id":request.request_id,"saved":false,"detail":e.to_string()}),
                        });
                    }
                };
                let mut names = self.name.subscribe();
                names.mark_changed();
                let name_work = async {
                    while names.changed().await.is_ok() {
                        let name = names.borrow_and_update().trim().to_owned();
                        if name.is_empty() {
                            continue;
                        }
                        // An older Home has no such operation; that is not an error for the phone.
                        let _ = tokio::time::timeout(
                            Duration::from_secs(3),
                            operations.invoke::<_, bool>(
                                &home.scoped_component(crate::mobile_pose::COMPONENT),
                                "name", "phone-name", &uuid::Uuid::new_v4().to_string(),
                                Duration::from_secs(2), &crate::mobile_pose::SetName { name },
                            ),
                        )
                        .await;
                    }
                };
                let mut geos = self.geo.subscribe();
                geos.mark_changed();
                let geo_work = async {
                    while geos.changed().await.is_ok() {
                        let Some(reference) = geos.borrow_and_update().clone() else {
                            continue;
                        };
                        // An older Home has no such operation, and a reference for another scene is refused.
                        let _ = tokio::time::timeout(
                            Duration::from_secs(3),
                            operations.invoke::<_, bool>(
                                &home.scoped_component(crate::mobile_pose::COMPONENT),
                                "geo", "phone-geo", &uuid::Uuid::new_v4().to_string(),
                                Duration::from_secs(2), &reference,
                            ),
                        )
                        .await;
                    }
                };
                let mut drive_input = self.drive.subscribe();
                let drive_work = async {
                    use futures::StreamExt;
                    let session = uuid::Uuid::new_v4().to_string();
                    let mut sequence = 0u64;
                    let mut held: Option<String> = None;
                    let mut stops_left = 0u8;
                    let mut tick = tokio::time::interval(Duration::from_millis(100));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    // Home can take a few hundred milliseconds to answer. Sends
                    // overlap instead of queueing behind each reply, so the stick
                    // still reaches Home ten times a second; Home orders them by
                    // sequence and ignores any that arrive late.
                    let mut in_flight = futures::stream::FuturesUnordered::new();
                    loop {
                        tokio::select! {
                            _ = tick.tick() => {},
                            changed = drive_input.changed() => if changed.is_err() { break },
                            Some(_) = in_flight.next(), if !in_flight.is_empty() => continue,
                        }
                        let input = drive_input.borrow_and_update().clone();
                        let (target, forward, turn) = match input {
                            Some((target, forward, turn)) => {
                                held = Some(target.clone());
                                stops_left = 5;
                                (target, forward, turn)
                            }
                            // On release, say "stop" several times so a lost
                            // message cannot leave the robot on its last command.
                            None => match (&held, stops_left) {
                                (Some(target), 1..) => {
                                    stops_left -= 1;
                                    (target.clone(), 0., 0.)
                                }
                                _ => {
                                    held = None;
                                    continue;
                                }
                            },
                        };
                        if in_flight.len() >= 16 {
                            continue; // Home is not answering; do not pile up requests.
                        }
                        sequence += 1;
                        let command = crate::mobile_drive::Command {
                            session: session.clone(),
                            sequence,
                            target_peer_id: target,
                            forward: forward.clamp(-1., 1.),
                            turn: turn.clamp(-1., 1.),
                        };
                        let component = home.scoped_component(crate::mobile_drive::COMPONENT);
                        let operations = &operations;
                        in_flight.push(async move {
                            let _ = tokio::time::timeout(
                                Duration::from_millis(1000),
                                operations.invoke::<_, bool>(
                                    &component, "update", "phone-drive",
                                    &uuid::Uuid::new_v4().to_string(),
                                    Duration::from_millis(900), &command,
                                ),
                            )
                            .await;
                        });
                    }
                };
                // Every pose, in order, into this phone's own Catalog. It does
                // not wait on Home, so the pose taken with a camera frame is
                // there under that frame's exact capture time. The first pose
                // also tells depth and the camera which AR session they are in.
                let mut pose_log = self.pose_log_receiver.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap();
                let pose_product_work = async {
                    while let Some(sample) = pose_log.recv().await {
                        let sample = match sample.map(|sample| products.timing.pose(sample)).transpose() {
                            Ok(sample) => sample,
                            Err(error) => { self.local_status(format!("Pose not shared: {error}")); continue; }
                        };
                        let result = {
                            let mut declared = declared();
                            let declared = &mut *declared;
                            match &sample {
                                Some(sample) => (|| {
                                    match declared.pose.as_mut() {
                                        Some(publisher) => publisher.publish(&home.peer_id, sample)?,
                                        None => {
                                            let mut publisher = crate::mobile_odometry::Publisher::new(&products, &home.peer_id, sample)?;
                                            publisher.publish(&home.peer_id, sample)?;
                                            declared.pose = Some(publisher);
                                        }
                                    }
                                    match declared.depth_image.as_mut() {
                                        Some(publisher) => publisher.enter_session(&sample.session, sample.timestamp_ns)?,
                                        None => declared.depth_image = Some(crate::mobile_depth_image::Publisher::new(&products, &sample.session)?),
                                    }
                                    if let Some(camera) = declared.camera.as_mut() {
                                        camera.enter_session(&sample.session, sample.timestamp_ns)?;
                                    }
                                    Ok::<(), anyhow::Error>(())
                                })(),
                                None => declared.pose.as_mut().map_or(Ok(()), |publisher| publisher.lost()),
                            }
                        };
                        if let Err(error) = result {
                            self.local_status(format!("Catalog not updated: {error}"));
                        }
                    }
                };
                let mut depth_images = self.depth_image.subscribe();
                let depth_image_work = async {
                    while depth_images.changed().await.is_ok() {
                        let Some((mut image, received)) = depth_images.borrow_and_update().clone() else { continue };
                        if received.elapsed() > Duration::from_millis(500) {
                            continue;
                        }
                        let result = (|| {
                            image.timestamp_ns = products.timing.capture(0, image.timestamp_ns)?;
                            image.alignment_timestamp_ns = products.timing.held(&image.session, image.alignment_timestamp_ns)?;
                            let mut declared = declared();
                            match declared.depth_image.as_mut() {
                                Some(publisher) => publisher.publish(image),
                                None => crate::mobile_depth_image::Publisher::new(&products, &image.session)
                                    .and_then(|mut publisher| {
                                        publisher.publish(image)?;
                                        declared.depth_image = Some(publisher);
                                        Ok(())
                                    }),
                            }
                        })();
                        // A bad image must not tear down mapping or the peer.
                        if let Err(error) = result {
                            self.local_status(format!("Depth image not shared: {error}"));
                        }
                    }
                };
                // Where this AR session sits in the scene is published as the app learns it,
                // and again on each connection. It does not wait for a depth capture.
                let mut placements = self.placement.subscribe();
                placements.mark_changed();
                let placement_work = async {
                    while placements.changed().await.is_ok() {
                        let latest = placements.borrow_and_update().clone();
                        let at = match latest.as_ref().map(|p| products.timing.assert_current_held(&p.session, p.timestamp_ns)).transpose() {
                            Ok(at) => at,
                            Err(error) => {
                                self.local_status(format!("Scene transform waiting for capture clock: {error}"));
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                placements.mark_changed();
                                continue;
                            }
                        };
                        let published = (|| {
                            let mut declared = declared();
                            let Some(placement) = latest else {
                                return declared.placed.as_mut().map_or(Ok(()), |placed| placed.lose());
                            };
                            if declared.placed.as_ref().is_some_and(|p| !p.serves(&placement.session, &home.peer_id, &home.frame))
                                && let Some(old) = declared.placed.take() {
                                old.close()?;
                            }
                            if declared.placed.is_none() {
                                declared.placed = Some(crate::mobile_declared::Publisher::new(&products, &placement.session, &home.peer_id, &home.frame)?);
                            }
                            declared.placed.as_mut().unwrap().align(at.unwrap(), &placement.session_to_scene)
                        })();
                        if let Err(error) = published { self.local_status(format!("Scene transform not published: {error}")); }
                    }
                };
                let mut depths = self.depth.subscribe();
                let depth_work = async {
                    let mut tick = tokio::time::interval(Duration::from_millis(500));
                    loop {
                        tick.tick().await;
                        if !depths.has_changed().unwrap_or(false) {
                            continue;
                        }
                        let latest = depths.borrow_and_update().clone();
                        let Some((sample, received)) = latest else {
                            // An app that does not say where it is apart from its captures has,
                            // with no capture, no alignment to vouch for either.
                            if self.placement.borrow().is_none() && let Some(placed) = declared().placed.as_mut() { let _ = placed.lose(); }
                            self.depth_status("Depth sharing paused".into());
                            continue;
                        };
                        if received.elapsed() > Duration::from_secs(2) {
                            continue;
                        }
                        let colored = sample.colors.is_some();
                        let count = sample.points.len();
                        // Published for any peer to read: where this AR session sits in the scene,
                        // and the points with no pose of their own. A reader places them from the
                        // phone's pose Product at the capture time. Nothing is pushed.
                        let published = (|| {
                            let mut declared = declared();
                            if declared.placed.as_ref().is_some_and(|p| !p.serves(&sample.session, &home.peer_id, &home.frame))
                                && let Some(old) = declared.placed.take() {
                                old.close()?;
                            }
                            if declared.placed.is_none() {
                                declared.placed = Some(crate::mobile_declared::Publisher::new(&products, &sample.session, &home.peer_id, &home.frame)?);
                            }
                            let placed = declared.placed.as_mut().unwrap();
                            let at = products.timing.capture(0, sample.timestamp_ns)?;
                            let aligned_at = products.timing.held(&sample.session, sample.alignment.timestamp_ns)?;
                            placed.align(aligned_at, &sample.alignment.session_to_home)?;
                            placed.capture(at, &sample.points, sample.colors.as_deref())
                        })();
                        self.depth_status(match published {
                            Ok(()) if colored => format!("Depth published · {count} colored voxels"),
                            Ok(()) => format!("Depth published · {count} voxels"),
                            Err(error) => format!("Depth not published: {error}"),
                        });
                    }
                };
                let mut local_publisher = None;
                let local_work = async {
                    let mut source_time = crate::mobile_products::RetainedTimestamp::default();
                    let mut tick = tokio::time::interval(Duration::from_secs(1));
                    // The map only grows (cells in the order first measured), so its
                    // size says whether it changed. Unchanged, it goes every 5 s,
                    // which keeps it inside Home's 10 s listing window.
                    let mut sent: Option<(usize, Instant)> = None;
                    loop {
                        tick.tick().await;
                        let latest = self.local_map.borrow().clone();
                        let Some((mut sample, received)) = latest else {
                            continue;
                        };
                        if received.elapsed() > Duration::from_secs(3) {
                            continue;
                        }
                        let cells = sample.cells.len();
                        if sent.is_some_and(|(count, at)| count == cells && at.elapsed() < Duration::from_secs(5)) {
                            continue;
                        }
                        let result = async {
                            sample.timestamp_ns = source_time.convert(&products.timing, &sample.session, sample.timestamp_ns)?;
                            if local_publisher.is_none() {
                                local_publisher = Some(crate::mobile_local::Publisher::new(&products)?);
                            }
                            let envelope = local_publisher.as_ref().unwrap().publish(sample)?;
                            let accepted = operations.invoke::<_,bool>(crate::mobile_local::RECEIVER,
                                "ingest","phone-local",&uuid::Uuid::new_v4().to_string(),Duration::from_secs(3),&envelope).await?;
                            ensure!(accepted,"Home rejected stale local map");
                            Ok::<(),anyhow::Error>(())
                        }
                        .await;
                        if result.is_ok() { sent = Some((cells, Instant::now())); }
                        self.local_status(match result {
                            Ok(()) => "Local voxel map received by Home (unaligned)".into(),
                            Err(e) => format!("Local map: {e}"),
                        });
                    }
                    #[allow(unreachable_code)]
                    Ok::<(), anyhow::Error>(())
                };
                let work = tokio::select! {
                    _ = self.capture_clock_reset.cancelled() => Err(anyhow::anyhow!("Capture clock reset; reconnect to establish a new SDK session clock")),
                    result = clock_work => result,
                    result = camera_work => result,
                    result = screen_work => result,
                    _ = change_work => Ok(()),
                    _ = portal_work => Ok(()),
                    _ = path_work => Ok(()),
                    _ = route_work => Ok(()),
                    _ = placement_work => Ok(()),
                    _ = origin_work => Ok(()),
                    _ = drive_work => Ok(()),
                    _ = name_work => Ok(()),
                    _ = geo_work => Ok(()),
                    result = local_work => result,
                    _ = pose_product_work => Ok(()),
                    _ = depth_image_work => Ok(()),
                    _ = depth_work => Ok(()),
                    _ = join_work => Ok(()),
                    _ = self.follow_geometry(&peer, &home) => Ok(()),
                    _ = self.follow_voxels(&peer, &home) => Ok(()),
                    result = self.follow(&peer, &home) => result,
                    _ = commands => Ok(()),
                };
                self.camera_demand.store(false, Ordering::Release);
                self.screen_demand.store(false, Ordering::Release);
                *self.timing.lock().unwrap_or_else(|e| e.into_inner()) = None;
                drop(onboarding_display);
                *self.connection.lock().unwrap_or_else(|e| e.into_inner()) = None;
                if let Some(screen) = screen.take() { let _ = screen.close(); }
                let Declared { camera, depth_image, pose: pose_publisher, placed } = std::mem::replace(
                    &mut *declared(), Declared { camera: None, depth_image: None, pose: None, placed: None });
                if let Some(publisher) = placed { let _ = publisher.close(); }
                if let Some(publisher) = depth_image { let _ = publisher.close(); }
                if let Some(camera) = camera { let _ = camera.close(); }
                if let Some(calibration)=calibration_publisher {let _=calibration.close();}
                if let Some(publisher) = local_publisher {
                    let _ = publisher.close().await;
                }
                if let Some(publisher) = pose_publisher {
                    let _ = publisher.close();
                }
                let cleanup = products.close().await;
                work.and(cleanup)
                }.await;
                self.status("stopping", "Closing P2P connection");
                let cleanup = peer.shutdown().await.map_err(anyhow::Error::from);
                work.and(cleanup)
            }
            Err(error) => Err(anyhow::anyhow!(startup_error(&error))),
        };
        bootstrap.session().close().await;
        result
    }

    fn set_geometry(&self, geometry: Option<serde_json::Value>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.geometry = geometry;
        if state.state == "live"
            && let Some(reference) = &state.reference
        {
            state.portals = Some(crate::mobile_geometry::merge(
                reference,
                state.geometry.as_ref(),
            ));
            state.version += 1;
        }
    }
    /// Home's voxels arrive as deltas on their own Product. A missed delta
    /// ends the subscription; the next one starts from Home's last keyframe.
    async fn follow_voxels(&self, peer: &AukiPeer, home: &HomeConnection) {
        if home.local_scene_id().is_none() {
            return std::future::pending().await;
        }
        loop {
            let result = crate::local_mobile::receive_voxels(peer, home, &self.stop, |update| {
                self.voxels.lock().unwrap_or_else(|e| e.into_inner()).apply(update)
            }).await;
            if self.stop.is_cancelled() {
                return;
            }
            if let Err(error) = result {
                self.local_status(format!("Voxels: {error}"));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn follow_geometry(&self, peer: &AukiPeer, home: &HomeConnection) {
        // Local scene Products already contain only their own geometry and notes.
        // Never subscribe to or overlay Xmart's separate legacy live map.
        if home.local_scene_id().is_some() {
            self.stop.cancelled().await;
            return;
        }
        loop {
            let attempt = async {
                let mut subscription =
                    crate::network::subscribe_scene_product(peer, home, "home-live-scene").await?;
                let clock = subscription.product().producer.clock.clone();
                let work = async {
                    loop {
                        let event = tokio::select! {
                            _ = self.stop.cancelled() => return Ok(()),
                            event = tokio::time::timeout(Duration::from_secs(5), subscription.next()) => event??,
                        };
                        match event {
                            Some(RemoteObservationEvent::Observation(observation)) => {
                                let scene = &observation.payload;
                                scene.validate(&home.peer_id)?;
                                ensure!(clock_ref(&scene.publication_clock) == clock, "Live scene clock mismatch");
                                self.set_geometry(Some(crate::mobile_geometry::render(scene)?));
                            }
                            Some(RemoteObservationEvent::Gap(_)) => {},
                            _ => bail!("Live geometry stream ended"),
                        }
                    }
                }.await;
                let cleanup = subscription.close().await.map_err(anyhow::Error::from);
                work.and(cleanup)
            };
            tokio::select! { _ = self.stop.cancelled() => break, _ = attempt => {} }
            self.set_geometry(None);
            tokio::select! { _ = self.stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
        }
    }

    async fn follow(&self, peer: &AukiPeer, home: &HomeConnection) -> Result<()> {
        let mut failures = 0;
        loop {
            if self.stop.is_cancelled() {
                return Ok(());
            }
            // A routine idle refresh is not a disconnect. Keep the current
            // reference/alignment until a real failure or replacement arrives.
            if self.snapshot().state != "live" {
                self.status("connecting", "Connecting to Home over authenticated P2P");
            }
            let result =
                receive_portals(peer, home, &self.stop, None, |portals| {
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.version += 1;
                    state.state = "live";
                    state.detail = "Home connected".into();
                    state.reference = Some(portals.clone());
                    state.portals = Some(crate::mobile_geometry::merge(
                        &portals,
                        state.geometry.as_ref(),
                    ));
                    failures = 0;
                })
                .await;
            if self.stop.is_cancelled() {
                return Ok(());
            }
            if let Err(error) = &result {
                // Say why, bounded and on one line; "Home unavailable" alone hid
                // format mismatches between a phone build and Home.
                let mut reason = format!("{error:#}").replace('\n', " ");
                reason.truncate(220);
                eprintln!("[peer] Home follow failed: {reason}");
                self.local_status(format!("Home follow failed: {reason}"));
                failures += 1;
                ensure!(
                    failures < 3,
                    "Home connection failed. Check Home is online, the profile routes are current, and both peers can reach each other. Reconnect to retry."
                );
                self.status(
                    "reconnecting",
                    format!("Home unavailable · retry {failures}/3"),
                );
                tokio::select! {
                    _ = self.stop.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(Duration::from_secs(2 * failures)) => {},
                }
            }
            // No idle refresh by default: a quiet scene is not a disconnect. The
            // subscription ends only on an error, which retries. Maps are full replacements.
        }
    }
}

/// One validated subscription, shared by the iPhone bridge and loopback tests.
/// Returns after Stop, or after `idle_refresh` without a change when one is set;
/// `None` never times out a live stream. Always closes its subscription.
pub async fn receive_portals(
    peer: &AukiPeer,
    home: &HomeConnection,
    stop: &CancellationToken,
    idle_refresh: Option<Duration>,
    mut accept: impl FnMut(serde_json::Value),
) -> Result<()> {
    if home.local_scene_id().is_some() {
        return crate::local_mobile::receive(peer, home, stop, idle_refresh, accept).await;
    }
    let mut subscription = tokio::select! {
        _ = stop.cancelled() => return Ok(()),
        result = tokio::time::timeout(Duration::from_secs(60), subscribe_scene(peer, home)) => result??,
    };
    let clock = subscription.product().producer.clock.clone();
    let work = async {
        let mut received = false;
        loop {
            let deadline = if !received {
                Some(Duration::from_secs(30))
            } else {
                idle_refresh
            };
            let event = tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                exit = peer.wait_stopped() => bail!("Phone peer stopped: {exit:?}"),
                event = crate::local_mobile::next_within(deadline, subscription.next()) => event,
            };
            let event = match event {
                Ok(event) => event?,
                Err(_) if received => return Ok(()),
                Err(_) => bail!("Home did not publish an initial scene"),
            };
            match event {
                Some(RemoteObservationEvent::Observation(observation)) => {
                    let scene = &observation.payload;
                    scene.validate(&home.peer_id)?;
                    ensure!(
                        scene.frame == home.frame,
                        "Scene and pairing profile frames differ"
                    );
                    ensure!(
                        clock_ref(&scene.publication_clock) == clock,
                        "Publication clock mismatch"
                    );
                    let mut mobile = serde_json::to_value(&scene.portals)?;
                    mobile["render"] = crate::scene_mirror::render_scene(scene)?;
                    accept(mobile);
                    received = true;
                }
                Some(RemoteObservationEvent::Gap(_)) => {} // next complete scene replaces the previous one
                Some(RemoteObservationEvent::Closed(reason)) => {
                    bail!("Home stream closed: {reason:?}")
                }
                None => bail!("Home stream ended"),
            }
        }
    }
    .await;
    let cleanup = subscription.close().await.map_err(anyhow::Error::from);
    work.and(cleanup)
}

#[cfg(test)]
mod capture_clock_tests {
    use super::*;
    #[test]
    fn onboarding_history_uses_capture_clock_without_changing_app_evidence() {
        let client = MobileClient::default();
        let timing = Arc::new(crate::mobile_products::CaptureTiming::new("phone"));
        timing.observe(0, 100_000_000, 50_100_000_010, 100_000_020).unwrap();
        timing.observe(0, 200_000_000, 50_200_000_010, 200_000_020).unwrap();
        let clock = timing.clock_reference().unwrap();
        *client.timing.lock().unwrap() = Some(timing);
        let session = "ar-session";
        for t in [125_000_000, 175_000_000] {
            client.update_pose(Some(crate::mobile_pose::Sample { session: session.into(),
                timestamp_ns: 50_000_000_000 + t,
                pose: auki_scenegraph::RigidTransform::identity("camera", "ar"), camera_configuration_id: None }));
        }
        assert_eq!(client.pose.borrow().as_ref().unwrap().timestamp_ns, 50_175_000_000);
        {
            let history = client.onboarding.history.lock().unwrap();
            assert!(history.at(&clock, session, 150_000_000).is_ok());
            assert!(history.at(&clock, session, 50_150_000_000).is_err());
        }
        client.update_pose(None);
        assert!(client.onboarding.history.lock().unwrap().at(&clock, session, 150_000_000).is_err());
    }
    #[test]
    fn bridge_bracket_rejects_a_replaced_clock_generation() {
        let client = MobileClient::default();
        let first = Arc::new(crate::mobile_products::CaptureTiming::new("phone"));
        let generation = first.generation;
        *client.timing.lock().unwrap() = Some(first);
        assert_eq!(client.clock_generation(), generation);
        assert!(client.session_now_ns(generation) > 0);
        assert!(client.observe_native_clock(generation, 0, 100, 1000, 120));
        *client.timing.lock().unwrap() = Some(Arc::new(crate::mobile_products::CaptureTiming::new("phone")));
        assert_ne!(client.clock_generation(), generation);
        assert_eq!(client.session_now_ns(generation), 0);
        assert!(!client.observe_native_clock(generation, 0, 200, 1100, 220));
        assert!(client.timing.lock().unwrap().as_ref().unwrap().capture(0, 1050).is_err());
    }
    #[test]
    fn native_clock_reset_requests_a_new_session_not_tracking_loss() {
        let client = MobileClient::default();
        let timing = Arc::new(crate::mobile_products::CaptureTiming::new("phone"));
        let generation = timing.generation;
        *client.timing.lock().unwrap() = Some(timing);
        client.update_pose(None);
        assert_eq!(client.clock_generation(), generation);
        assert!(!client.capture_clock_reset.is_cancelled());
        assert!(client.observe_native_clock(generation, 0, 100, 1000, 120));
        assert!(!client.observe_native_clock(generation, 0, 200, 10_000_000, 220));
        assert!(client.capture_clock_reset.is_cancelled());
        let replacement = crate::mobile_products::CaptureTiming::new("phone");
        assert_ne!(replacement.generation, generation);
    }
}
