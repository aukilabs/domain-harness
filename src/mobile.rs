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
    camera: tokio::sync::watch::Sender<Option<(u64, Vec<u8>, Instant)>>,
    camera_active: AtomicBool,
    camera_demand: AtomicBool,
    local_map: tokio::sync::watch::Sender<Option<(crate::mobile_local::Snapshot, Instant)>>,
    depth: tokio::sync::watch::Sender<Option<(crate::mobile_depth::Sample, std::time::Instant)>>,
    pose: tokio::sync::watch::Sender<Option<crate::mobile_pose::Sample>>,
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
    portal_reports: tokio::sync::mpsc::Sender<crate::mobile_portal::Report>,
    portal_receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::mobile_portal::Report>>>,
    receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::mobile_notes::PlaceNote>>>,
}
impl Default for MobileClient {
    fn default() -> Self {
        let (commands, receiver) = tokio::sync::mpsc::channel(1);
        let (changes, change_receiver) = tokio::sync::mpsc::channel(1);
        let (portal_reports, portal_receiver) = tokio::sync::mpsc::channel(4);
        Self {
            changes,
            change_receiver: Mutex::new(Some(change_receiver)),
            portal_reports,
            portal_receiver: Mutex::new(Some(portal_receiver)),
            camera: tokio::sync::watch::channel(None).0,
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
        if self.camera_active.load(Ordering::Acquire) && self.camera_demand.load(Ordering::Acquire) {
            self.camera.send_replace(Some((timestamp, jpeg, Instant::now())));
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
    pub fn update_pose(&self, sample: Option<crate::mobile_pose::Sample>) {
        self.pose.send_replace(sample);
    }
    /// The phone paused its camera. Home keeps the last pose, marked asleep.
    pub fn set_asleep(&self, asleep: bool) {
        self.asleep.store(asleep, Ordering::Release);
        if asleep {
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
    /// Queue a newly seen Portal for Home to add to the scene.
    pub fn report_portal(&self, report: crate::mobile_portal::Report) -> bool {
        self.snapshot().state == "live" && self.portal_reports.try_send(report).is_ok()
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
        email: String,
        password: String,
        identity: String,
    ) {
        let result = self
            .connect(profile, email, password, Path::new(&identity))
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
        email: String,
        password: String,
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
        ensure!(
            !email.trim().is_empty() && !password.is_empty(),
            "Enter your Auki User email and password"
        );
        if self.stop.is_cancelled() {
            return Ok(());
        }
        let config =
            AukiPeerConfig::dev().with_listen_addresses(["/ip4/0.0.0.0/tcp/0".parse()?])?;
        let auth = AukiPeerBootstrap::authenticate(
            AuthClient::new(AuthEnvironment::dev())?,
            Credentials::user_password(email, password),
            config,
        );
        let bootstrap = tokio::select! {
            _ = self.stop.cancelled() => return Ok(()),
            result = tokio::time::timeout(Duration::from_secs(45), auth) =>
                result.map_err(|_| anyhow::anyhow!("Sign-in timed out"))?
                    .map_err(|_| anyhow::anyhow!("Auki User sign-in failed. Check your account and network."))?,
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
                let mut camera = None;
                let mut frames = self.camera.subscribe();
                let camera_work = async {
                    let mut tick = tokio::time::interval(Duration::from_millis(100));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        tick.tick().await;
                        let active = self.camera_active.load(Ordering::Acquire);
                        if active && camera.is_none() {
                            camera = Some(crate::mobile_camera::Camera::new(&products)?);
                        }
                        let demand = active && camera.as_ref().is_some_and(|c| c.demanded());
                        self.camera_demand.store(demand, Ordering::Release);
                        if frames.has_changed().unwrap_or(false) {
                            let latest = frames.borrow_and_update().clone();
                            if demand && let Some((timestamp, jpeg, received)) = latest
                                && received.elapsed() < Duration::from_millis(500) {
                                // Bad/late frames must not tear down mapping or the peer.
                                let _ = camera.as_mut().unwrap().publish(timestamp, jpeg);
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
                let mut poses = self.pose.subscribe();
                let mut changes = self.change_receiver.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap();
                let mut portal_reports = self.portal_receiver.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap();
                let portal_work = async {
                    while let Some(report) = portal_reports.recv().await {
                        let code = report.code.clone();
                        let (done, doing) = if report.replace { ("moved in", "moved") } else { ("added to", "added") };
                        let result = tokio::time::timeout(
                            Duration::from_secs(4),
                            operations.invoke::<_, bool>(
                                &home.scoped_component(crate::mobile_portal::COMPONENT),
                                "add", "phone-portal", &uuid::Uuid::new_v4().to_string(),
                                Duration::from_secs(3), &report,
                            ),
                        )
                        .await;
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
                let pose_work = async {
                    // Latest-only watch channel: send immediately, never queue old poses.
                    loop {
                        // While asleep, repeat the status so Home keeps the phone listed.
                        let asleep = self.asleep.load(Ordering::Acquire);
                        tokio::select! {
                            changed = poses.changed() => if changed.is_err() { break },
                            _ = tokio::time::sleep(Duration::from_secs(20)), if asleep => {}
                        }
                        let sample = poses.borrow_and_update().clone();
                        let status = (sample.is_none() && self.asleep.load(Ordering::Acquire))
                            .then(|| "asleep".to_string());
                        let _ = tokio::time::timeout(
                            Duration::from_secs(2),
                            operations.invoke::<_,bool>(&home.scoped_component(crate::mobile_pose::COMPONENT),
                                "update","phone-pose",&uuid::Uuid::new_v4().to_string(),Duration::from_secs(1),
                                &crate::mobile_pose::Update { sample, status }),
                        )
                        .await;
                    }
                };
                let mut depth_publisher: Option<crate::mobile_depth::Publisher> = None;
                let mut depths = self.depth.subscribe();
                // An older Home rejects the optional color field. Only repeated
                // failures pause color, and it is offered again every minute, so a
                // brief network error cannot switch color off for the session.
                let mut send_colors = true;
                let mut color_failures = 0u32;
                let mut retry_color_at = Instant::now();
                let depth_work = async {
                    let mut tick = tokio::time::interval(Duration::from_millis(500));
                    loop {
                        tick.tick().await;
                        if !depths.has_changed().unwrap_or(false) {
                            continue;
                        }
                        let latest = depths.borrow_and_update().clone();
                        let Some((mut sample, received)) = latest else {
                            self.depth_status("Depth sharing paused".into());
                            continue;
                        };
                        if received.elapsed() > Duration::from_secs(2) {
                            continue;
                        }
                        if !send_colors && Instant::now() >= retry_color_at {
                            send_colors = true;
                        }
                        if !send_colors {
                            sample.colors = None;
                        }
                        let colored = sample.colors.is_some();
                        let count = sample.points.len();
                        let result = async {
                            if let Some(publisher) = depth_publisher.as_mut()
                                && publisher.session != sample.session {
                                publisher.reconfigure(&sample.session, sample.timestamp_ns)?;
                            }
                            if depth_publisher.is_none() {
                                depth_publisher = Some(crate::mobile_depth::Publisher::new(
                                    &products,
                                    &sample.session,
                                )?);
                            }
                            let envelope = depth_publisher.as_ref().unwrap().publish(sample)?;
                            let accepted = tokio::time::timeout(
                                Duration::from_secs(2),
                                operations.invoke::<_,bool>(&home.scoped_component(crate::mobile_depth::COMPONENT),
                                    "ingest","phone-depth",&uuid::Uuid::new_v4().to_string(),Duration::from_secs(1),&envelope),
                            )
                            .await??;
                            ensure!(accepted, "Home rejected stale depth sample");
                            Ok::<_, anyhow::Error>(())
                        }
                        .await;
                        if colored {
                            if result.is_ok() {
                                color_failures = 0;
                            } else {
                                color_failures += 1;
                                if color_failures >= 3 {
                                    send_colors = false;
                                    retry_color_at = Instant::now() + Duration::from_secs(60);
                                    // One more failure after the retry pauses it again.
                                    color_failures = 2;
                                }
                            }
                        }
                        self.depth_status(match result {
                            Ok(()) if colored => format!("Depth sent to Home · {count} colored voxels"),
                            Ok(()) => format!("Depth sent to Home · {count} voxels"),
                            Err(error) if colored && !send_colors => format!("Color paused for a minute after repeated failures: {error}"),
                            Err(error) => format!("Depth not sent: {error}"),
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
                    _ = drive_work => Ok(()),
                    _ = name_work => Ok(()),
                    _ = geo_work => Ok(()),
                    result = local_work => result,
                    _ = pose_work => Ok(()),
                    _ = depth_work => Ok(()),
                    _ = self.follow_geometry(&peer, &home) => Ok(()),
                    result = self.follow(&peer, &home) => result,
                    _ = commands => Ok(()),
                };
                self.camera_demand.store(false, Ordering::Release);
                if let Some(camera) = camera { let _ = camera.close(); }
                if let Some(publisher) = local_publisher {
                    let _ = publisher.close().await;
                }
                if let Some(publisher) = depth_publisher {
                    let _ = publisher.close().await;
                }
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    crate::mobile_pose::send(
                        &peer,
                        &home,
                        // A phone that locks while asleep stays listed as asleep.
                        &crate::mobile_pose::Update {
                            sample: None,
                            status: self.asleep.load(Ordering::Acquire).then(|| "asleep".to_string()),
                        },
                    ),
                )
                .await;
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
