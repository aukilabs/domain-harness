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
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

pub const SURVEY_QUEUE_CAPACITY: usize = 512;

// AuthError's Display contract excludes credentials, tokens and response bodies.
// Do not print source chains: identity/transport sources may contain local data.
fn startup_error(error: &auki_sdk::AukiPeerBootstrapError) -> String {
    use auki_sdk::AukiPeerBootstrapError::*;
    match error {
        ConfigureAuthentication(e) => format!("Phone auth configuration failed: {e}"),
        Authenticate(e) => format!("Phone sign-in failed: {e}"),
        ListDomains(e) => format!("Phone Domain lookup failed: {e}"),
        AuthorizePeer(e) => format!("Phone Domain authorization failed: {e}"),
        LoadIdentity(_) => "Phone identity could not be loaded or saved in app-private storage".into(),
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
            let reason: String = reason.chars().filter(|c| !c.is_control()).take(512).collect();
            format!("{stage}: {reason}")
        }
        Error::Io(error) => format!("local I/O: {:?} (OS code {:?})", error.kind(), error.raw_os_error()),
        Error::RuntimeUnavailable => "Tokio runtime unavailable".into(),
        Error::SwarmStopped => "transport worker stopped".into(),
        // A discriminant identifies other variants without printing any payload,
        // in particular token, key, identity or remote response material.
        other => format!("SDK transport error {:?} (details withheld)", std::mem::discriminant(other)),
    }
}

#[cfg(test)]
mod startup_error_tests {
    use super::*;
    use auki_sdk::{AuthError, AukiPeerBootstrapError as Error, AukiPeerStartError};

    #[test]
    fn distinguishes_denial_network_and_local_startup() {
        assert_eq!(startup_error(&Error::AuthorizePeer(AuthError::DomainNotAccessible)),
            "Phone Domain authorization failed: selected Domain is not accessible");
        assert_eq!(startup_error(&Error::AuthorizePeer(AuthError::HttpStatus {
            endpoint: "peer authorization", status: 503,
        })), "Phone Domain authorization failed: peer authorization returned HTTP 503");
        assert_eq!(startup_error(&Error::StartPeer(AukiPeerStartError::ListenerStartupTimeout)),
            "Phone P2P startup failed: the authenticated P2P listeners did not bind within the startup deadline");
    }

    #[test]
    fn transport_diagnostic_is_specific_and_omits_sensitive_payloads() {
        let error = Error::StartPeer(AukiPeerStartError::Transport(
            auki_p2p::Error::TransportBuild("failed to read /etc/resolv.conf\n".into()),
        ));
        assert_eq!(startup_error(&error),
            "Phone transport startup failed: transport construction: failed to read /etc/resolv.conf");
        assert!(!transport_error(&auki_p2p::Error::InvalidToken("secret-token".into())).contains("secret-token"));
        assert!(!transport_error(&auki_p2p::Error::Io(std::io::Error::other("private-path"))).contains("private-path"));
        assert!(transport_error(&auki_p2p::Error::TransportBuild("x".repeat(4096))).len() < 550);
    }
}

#[derive(Clone, Serialize)]
pub struct MobileState {
    pub depth_detail: String,
    #[serde(skip)]
    reference: Option<serde_json::Value>,
    #[serde(skip)]
    geometry: Option<serde_json::Value>,
    pub annotation: Option<serde_json::Value>,
    pub survey: Option<serde_json::Value>,
    pub survey_pending: usize,
    pub version: u64,
    pub state: &'static str,
    pub detail: String,

    /// Validated Portal snapshot plus renderable scene geometry, in Home metres.
    pub portals: Option<serde_json::Value>,
}

pub struct MobileClient {
    local_map: tokio::sync::watch::Sender<Option<(crate::mobile_local::Snapshot, Instant)>>,
    depth: tokio::sync::watch::Sender<Option<(crate::mobile_depth::Sample, std::time::Instant)>>,
    pose: tokio::sync::watch::Sender<Option<crate::mobile_pose::Sample>>,
    stop: CancellationToken,
    state: Mutex<MobileState>,
    finished: AtomicBool,
    commands: tokio::sync::mpsc::Sender<crate::mobile_notes::PlaceNote>,
    receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::mobile_notes::PlaceNote>>>,
    survey: tokio::sync::mpsc::Sender<crate::mobile_survey::Sample>,
    survey_receiver: Mutex<Option<tokio::sync::mpsc::Receiver<crate::mobile_survey::Sample>>>,
    survey_inflight: AtomicUsize,
}
impl Default for MobileClient {
    fn default() -> Self {
        let (commands, receiver) = tokio::sync::mpsc::channel(1);
        let (survey, survey_receiver) = tokio::sync::mpsc::channel(SURVEY_QUEUE_CAPACITY);
        Self {
            local_map: tokio::sync::watch::channel(None).0,
            pose: tokio::sync::watch::channel(None).0,
            depth: tokio::sync::watch::channel(None).0,
            commands,
            receiver: Mutex::new(Some(receiver)),
            survey,
            survey_receiver: Mutex::new(Some(survey_receiver)),
            survey_inflight: AtomicUsize::new(0),
            stop: CancellationToken::new(),
            state: Mutex::new(MobileState {
                version: 1,
                depth_detail: "Depth not shared yet".into(),
                reference: None,
                geometry: None,
                annotation: None,
                survey: None,
                survey_pending: 0,
                state: "connecting",
                detail: "Signing in to Auki development services".into(),

                portals: None,
            }),
            finished: AtomicBool::new(false),
        }
    }
}
impl MobileClient {
    pub fn update_local_map(&self, sample: Option<crate::mobile_local::Snapshot>) {
        self.local_map.send_replace(sample.map(|s| (s, Instant::now())));
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
    pub fn update_pose(&self, sample: Option<crate::mobile_pose::Sample>) {
        self.pose.send_replace(sample);
    }
    pub fn annotate(&self, request: crate::mobile_notes::PlaceNote) -> bool {
        if self.snapshot().state != "live" {
            return false;
        }
        self.commands.try_send(request).is_ok()
    }
    pub fn survey(&self, sample: crate::mobile_survey::Sample) -> bool {
        if self.snapshot().state != "live" {
            return false;
        }
        self.survey.try_send(sample).is_ok()
    }
    pub fn stop(&self) {
        self.stop.cancel();
    }
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
    async fn publish_survey(
        &self,
        peer: &AukiPeer,
        home: &HomeConnection,
        sample: &crate::mobile_survey::Sample,
        cache: &mut Option<crate::mobile_survey::SurveyTarget>,
    ) {
        self.survey_inflight.store(1, Ordering::Relaxed);
        let result = crate::mobile_survey::send(peer, home, sample, cache).await;
        self.survey_inflight.store(0, Ordering::Relaxed);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.version += 1;
        state.survey = Some(match result {
            Ok(()) => serde_json::json!({"id":sample.id,"saved":true}),
            Err(error) => serde_json::json!({
                "id":sample.id,
                "saved":false,
                "detail":error.to_string()
            }),
        });
    }

    pub fn snapshot(&self) -> MobileState {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let queued = self
            .survey
            .max_capacity()
            .saturating_sub(self.survey.capacity());
        state.survey_pending = queued + self.survey_inflight.load(Ordering::Relaxed);
        state
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
        let result = Arc::clone(&self)
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
        self: Arc<Self>,
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
        ensure!(
            home.frame == crate::model::FRAME && home.product_id == crate::network::SCENE_PRODUCT,
            "Expected Home's OpenGL canonical scene"
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
        let config = AukiPeerConfig::dev()
            .with_listen_addresses(["/ip4/0.0.0.0/tcp/0".parse()?])?;
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
                let peer = Arc::new(peer);
                let mut receiver = self
                    .receiver
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                    .unwrap();
                let commands = async {
                    while let Some(request) = receiver.recv().await {
                        let result = crate::mobile_notes::send(peer.as_ref(), &home, &request).await;
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
                let mut survey_receiver = self
                    .survey_receiver
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                    .unwrap();
                let survey_peer = Arc::clone(&peer);
                let survey_home = home.clone();
                let survey_client = Arc::clone(&self);
                let mut survey_handle = tokio::spawn(async move {
                    let mut survey_target = None;
                    loop {
                        let sample = tokio::select! {
                            sample = survey_receiver.recv() => sample,
                            _ = survey_client.stop.cancelled() => {
                                while let Ok(sample) = survey_receiver.try_recv() {
                                    survey_client.publish_survey(survey_peer.as_ref(), &survey_home, &sample, &mut survey_target).await;
                                }
                                break;
                            }
                        };
                        let Some(sample) = sample else {
                            break;
                        };
                        survey_client.publish_survey(survey_peer.as_ref(), &survey_home, &sample, &mut survey_target).await;
                    }
                });
                let mut poses = self.pose.subscribe();
                let pose_work = async {
                    let mut tick = tokio::time::interval(Duration::from_millis(500));
                    loop {
                        tick.tick().await;
                        if !poses.has_changed().unwrap_or(false) {
                            continue;
                        }
                        let sample = poses.borrow_and_update().clone();
                        let _ = tokio::time::timeout(
                            Duration::from_secs(2),
                            crate::mobile_pose::send(
                                peer.as_ref(),
                                &home,
                                &crate::mobile_pose::Update { sample },
                            ),
                        )
                        .await;
                    }
                };
                let mut depth_publisher: Option<crate::mobile_depth::Publisher> = None;
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
                            self.depth_status("Depth sharing paused".into());
                            continue;
                        };
                        if received.elapsed() > Duration::from_secs(2) {
                            continue;
                        }
                        let count = sample.points.len();
                        let result = async {
                            if depth_publisher
                                .as_ref()
                                .is_some_and(|p| p.session != sample.session)
                                && let Some(old) = depth_publisher.take()
                            {
                                old.close().await?;
                            }
                            if depth_publisher.is_none() {
                                depth_publisher = Some(crate::mobile_depth::Publisher::new(
                                    peer.as_ref(),
                                    &sample.session,
                                )?);
                            }
                            let envelope = depth_publisher.as_ref().unwrap().publish(sample)?;
                            tokio::time::timeout(
                                Duration::from_secs(2),
                                crate::mobile_depth::send(peer.as_ref(), &home, &envelope),
                            )
                            .await??;
                            Ok::<_, anyhow::Error>(())
                        }
                        .await;
                        self.depth_status(match result {
                            Ok(()) => format!("Depth sent to Home · {count} points"),
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
                        let Some((sample, received)) = latest else { continue };
                        if received.elapsed() > Duration::from_secs(3) { continue; }
                        let result = async {
                            if local_publisher.is_none() { local_publisher = Some(crate::mobile_local::Publisher::new(peer.as_ref())?); }
                            let envelope = local_publisher.as_ref().unwrap().publish(sample)?;
                            crate::mobile_local::send(peer.as_ref(), &home, &envelope).await
                        }.await;
                        self.depth_status(match result { Ok(()) => "Local voxel map shared with Home (unaligned)".into(), Err(e) => format!("Local map: {e}") });
                    }
                    #[allow(unreachable_code)] Ok::<(), anyhow::Error>(())
                };
                let work = tokio::select! {
                    result = local_work => result,
                    _ = pose_work => Ok(()),
                    _ = depth_work => Ok(()),
                    _ = self.follow_geometry(peer.as_ref(), &home) => Ok(()),
                    result = self.follow(peer.as_ref(), &home) => result,
                    _ = commands => Ok(()),
                };
                self.stop.cancel();
                tokio::select! {
                    _ = &mut survey_handle => {}
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {
                        survey_handle.abort();
                        let _ = survey_handle.await;
                    }
                }
                if let Some(publisher) = local_publisher { let _ = publisher.close().await; }
                if let Some(publisher) = depth_publisher {
                    let _ = publisher.close().await;
                }
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    crate::mobile_pose::send(
                        peer.as_ref(),
                        &home,
                        &crate::mobile_pose::Update { sample: None },
                    ),
                )
                .await;
                self.status("stopping", "Closing P2P connection");
                let cleanup = match Arc::try_unwrap(peer) {
                    Ok(peer) => peer.shutdown().await.map_err(anyhow::Error::from),
                    Err(_) => Ok(()),
                };
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
            self.status("connecting", "Connecting to Home over authenticated P2P");
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
            if result.is_err() {
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
