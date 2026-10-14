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
    pub version: u64,
    pub state: &'static str,
    pub detail: String,

    /// Validated Portal snapshot plus renderable scene geometry, in Home metres.
    pub portals: Option<serde_json::Value>,
}

pub struct MobileClient {
    camera: tokio::sync::watch::Sender<Option<(u64, Vec<u8>, Instant, Option<crate::mobile_camera::Geometry>)>>,
    camera_configuration:tokio::sync::watch::Sender<Option<crate::mobile_camera::RegisteredConfiguration>>,
    camera_active: AtomicBool,
    camera_demand: AtomicBool,
    local_map: tokio::sync::watch::Sender<Option<(crate::mobile_local::Snapshot, Instant)>>,
    depth: tokio::sync::watch::Sender<Option<(crate::mobile_depth::Sample, std::time::Instant)>>,
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
    origin_reports: tokio::sync::mpsc::Sender<crate::scene_origin::Observation>,
    origin_receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::scene_origin::Observation>>>,
    receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::mobile_notes::PlaceNote>>>,
}
impl Default for MobileClient {
    fn default() -> Self {
        let (commands, receiver) = tokio::sync::mpsc::channel(1);
        let (changes, change_receiver) = tokio::sync::mpsc::channel(1);
        let (portal_reports, portal_receiver) = tokio::sync::mpsc::channel(4);
        let (path_changes, path_receiver) = tokio::sync::mpsc::channel(16);
        let (pose_log, pose_log_receiver) = tokio::sync::mpsc::channel(256);
        let (origin_reports, origin_receiver) = tokio::sync::mpsc::channel(1);
        Self {
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
            camera: tokio::sync::watch::channel(None).0,
            camera_configuration:tokio::sync::watch::channel(None).0,
            camera_active: AtomicBool::new(false),
            camera_demand: AtomicBool::new(false),
            local_map: tokio::sync::watch::channel(None).0,
            pose: tokio::sync::watch::channel(None).0,
            asleep: AtomicBool::new(false),
            drive: tokio::sync::watch::channel(None).0,
            name: tokio::sync::watch::channel(String::new()).0,
            geo: tokio::sync::watch::channel(None).0,
            depth: tokio::sync::watch::channel(None).0,
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
                state: "connecting",
                detail: "Signing in to Auki development services".into(),

                portals: None,
            }),
            finished: AtomicBool::new(false),
        }
    }
}
impl MobileClient {
    pub fn camera_demanded(&self, active: bool) -> bool {
        self.camera_active.store(active, Ordering::Release);
        if !active { self.camera.send_replace(None); }
        active && self.camera_demand.load(Ordering::Acquire) && !self.stop.is_cancelled()
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
    }
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
    pub fn snapshot(&self) -> MobileState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
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

    pub async fn run(
        self: Arc<Self>,
        profile: String,
        sign_in: SignIn,
        identity: String,
    ) {
        let result = self
            .connect(profile, sign_in, Path::new(&identity))
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
        let config =
            AukiPeerConfig::dev().with_listen_addresses(["/ip4/0.0.0.0/tcp/0".parse()?])?;
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
                let operations = crate::mobile_home::HomeOperations::new(&peer,&home)?;
                // Warm once before high-rate producers start, so their short
                // sample deadlines cannot repeatedly cancel cold discovery.
                tokio::select! { _ = self.stop.cancelled() => return Ok(()), _ = operations.warm() => {} }
                let products = crate::mobile_products::MobileProducts::mount(&peer)?;
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
                                let _ = match geometry {
                                    Some(geometry) => camera.publish_calibrated(timestamp, jpeg, geometry),
                                    None => camera.publish(timestamp, jpeg),
                                };
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
                        // The app matches these on the path's ID.
                        self.local_status(match result {
                            Ok(Ok(true)) => format!("Path {id} {done}"),
                            Ok(Ok(false)) => format!("Path {id} not {doing}: Home refused it"),
                            Ok(Err(error)) => format!("Path {id} not {doing}: {error}"),
                            Err(_) => format!("Path {id} not {doing}: Home did not answer"),
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
                        let Some((image, received)) = depth_images.borrow_and_update().clone() else { continue };
                        if received.elapsed() > Duration::from_millis(500) {
                            continue;
                        }
                        let result = {
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
                        };
                        // A bad image must not tear down mapping or the peer.
                        if let Err(error) = result {
                            self.local_status(format!("Depth image not shared: {error}"));
                        }
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
                            // No capture means no alignment to vouch for either.
                            if let Some(placed) = declared().placed.as_mut() { let _ = placed.lose(); }
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
                            placed.align(sample.alignment.timestamp_ns, &sample.alignment.session_to_home)?;
                            placed.capture(sample.timestamp_ns, &sample.points, sample.colors.as_deref())
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
                    let mut tick = tokio::time::interval(Duration::from_secs(1));
                    loop {
                        tick.tick().await;
                        let latest = self.local_map.borrow().clone();
                        let Some((sample, received)) = latest else {
                            continue;
                        };
                        if received.elapsed() > Duration::from_secs(3) {
                            continue;
                        }
                        let result = async {
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
                        self.local_status(match result {
                            Ok(()) => "Local voxel map received by Home (unaligned)".into(),
                            Err(e) => format!("Local map: {e}"),
                        });
                    }
                    #[allow(unreachable_code)]
                    Ok::<(), anyhow::Error>(())
                };
                let work = tokio::select! {
                    result = camera_work => result,
                    _ = change_work => Ok(()),
                    _ = portal_work => Ok(()),
                    _ = path_work => Ok(()),
                    _ = origin_work => Ok(()),
                    _ = drive_work => Ok(()),
                    _ = name_work => Ok(()),
                    _ = geo_work => Ok(()),
                    result = local_work => result,
                    _ = pose_product_work => Ok(()),
                    _ = depth_image_work => Ok(()),
                    _ = depth_work => Ok(()),
                    _ = self.follow_geometry(&peer, &home) => Ok(()),
                    result = self.follow(&peer, &home) => result,
                    _ = commands => Ok(()),
                };
                self.camera_demand.store(false, Ordering::Release);
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
                receive_portals(peer, home, &self.stop, Duration::from_secs(45), |portals| {
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
            // Renew an idle subscription to detect silent partitions and discover
            // a replacement Product after Home restarts. Maps are full replacements.
        }
    }
}

/// One validated subscription, shared by the iPhone bridge and loopback tests.
/// Returns after an idle refresh deadline or Stop; always closes its subscription.
pub async fn receive_portals(
    peer: &AukiPeer,
    home: &HomeConnection,
    stop: &CancellationToken,
    idle_refresh: Duration,
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
                Duration::from_secs(30)
            } else {
                idle_refresh
            };
            let event = tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                exit = peer.wait_stopped() => bail!("Phone peer stopped: {exit:?}"),
                event = tokio::time::timeout(deadline, subscription.next()) => event,
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
