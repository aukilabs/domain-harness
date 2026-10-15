//! Onboarding a robot from a phone (onboarding.md), robot side.
//!
//! The camera detector hands this module every onboarding code it reads,
//! over an in-process channel: never in its published detections, since an
//! invitation may carry a Wi-Fi password. On an invitation for this robot's
//! Home, the robot joins the Wi-Fi if asked, names itself in Home, asks the
//! phone to show its 50 mm marker, and times the phone's clock against its
//! own SDK session clock (native BBOS captures are explicitly mapped into it).
//! Each sighting of the marker becomes "where this camera was in the scene":
//!
//!   T_H_K(t) = T_H_M(t_phone) × inverse(T_K_M(t))
//!
//! The alignment worker turns those into the robot's held transform into
//! the scene, once enough agree, and publishes it like a Portal alignment.
use crate::onboarding as ob;
use anyhow::{Context, Result, anyhow, bail, ensure};
use auki_component_protocol::{CatalogResponse, ComponentProtocolClient};
use auki_odometry::Pose;
use auki_sdk::{Multiaddr, PeerId};
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

/// Codes one camera frame showed that matter to onboarding.
pub struct Seen {
    pub timestamp_ns: u64,
    pub codes: Vec<(String, [[f64; 2]; 4])>,
}
static CODES: OnceLock<tokio::sync::mpsc::UnboundedSender<Seen>> = OnceLock::new();
/// The head camera's calibration, set when the camera is mounted.
pub static CALIBRATION: Mutex<Option<crate::camera::Calibration>> = Mutex::new(None);

/// True for a code onboarding wants: an invitation, or a session marker.
pub fn wanted(payload: &str) -> bool {
    payload.starts_with(ob::PREFIX) || payload.starts_with("ONB:")
}
/// From the detector thread: never blocks, drops nothing it was not asked for.
pub fn offer(timestamp_ns: u64, codes: Vec<(String, [[f64; 2]; 4])>) {
    if let Some(tx) = CODES.get() && !codes.is_empty() {
        let _ = tx.send(Seen { timestamp_ns, codes });
    }
}

/// A marker sighting, already placed in the scene, for the alignment worker.
pub struct Sighting {
    pub timestamp_ns: u64,
    pub camera_in_home: Pose,
    pub timing_error_m: f64,
    pub range_m: f64,
}
#[derive(Default)]
pub struct Shared {
    pub sightings: Mutex<VecDeque<Sighting>>,
    /// Set by the alignment worker once sightings agreed and it published.
    pub agreed: Mutex<Option<u64>>,
}
pub static SHARED: LazyLock<Shared> = LazyLock::new(Shared::default);

/// Another peer's Component operations, found through its Catalog.
struct Remote {
    client: ComponentProtocolClient,
    peer: PeerId,
    route: Multiaddr,
    components: std::collections::HashMap<String, auki_components::ComponentReference>,
}
impl Remote {
    async fn connect(client: ComponentProtocolClient, peer: &str, routes: &[String]) -> Result<Self> {
        let peer: PeerId = peer.parse()?;
        let mut last = anyhow!("No route");
        for route in routes {
            let route: Multiaddr = route.parse()?;
            match tokio::time::timeout(Duration::from_secs(5), client.catalog_exact(peer, route.clone(), None)).await {
                Ok(Ok(CatalogResponse::Snapshot { snapshot })) => {
                    let components = snapshot.components.into_iter()
                        .map(|c| (c.manifest.component_id.clone(), c.manifest.reference())).collect();
                    return Ok(Self { client, peer, route, components });
                }
                Ok(Ok(_)) => last = anyhow!("Catalog was not a snapshot"),
                Ok(Err(e)) => last = e.into(),
                Err(_) => last = anyhow!("Catalog timed out"),
            }
        }
        Err(last)
    }
    async fn invoke<I: serde::Serialize, R: serde::de::DeserializeOwned>(&self, component: &str, operation: &str, request: &I) -> Result<R> {
        let reference = self.components.get(component).cloned().ok_or_else(|| anyhow!("{component} is not offered"))?;
        let reply = tokio::time::timeout(Duration::from_secs(4), self.client.invoke_exact::<_, R>(
            self.peer, self.route.clone(), reference, operation, "robot-onboarding",
            &uuid::Uuid::new_v4().to_string(), Some(Duration::from_secs(3)), request)).await
            .map_err(|_| anyhow!("{operation} timed out"))??;
        Ok(reply.result)
    }
}

/// Join the invitation's Wi-Fi unless already on it. Uses NetworkManager
/// through `sudo -n nmcli` (needs a sudoers entry for exactly that command);
/// arguments are passed without a shell and the password is never logged.
async fn join_wifi(wifi: &ob::Wifi) -> Result<()> {
    let current = tokio::process::Command::new("nmcli").args(["-t", "-f", "ACTIVE,SSID", "dev", "wifi"]).output().await
        .context("nmcli unavailable")?;
    let on_it = String::from_utf8_lossy(&current.stdout).lines().any(|l| l == format!("yes:{}", wifi.ssid));
    if on_it {
        return Ok(());
    }
    crate::diagnostic_log!("Onboarding: joining Wi-Fi {}", wifi.ssid);
    let mut command = tokio::process::Command::new("sudo");
    command.args(["-n", "nmcli", "dev", "wifi", "connect", &wifi.ssid]);
    if let Some(password) = &wifi.password { command.args(["password", password]); }
    if wifi.hidden { command.args(["hidden", "yes"]); }
    let output = tokio::time::timeout(Duration::from_secs(45), command.output()).await
        .map_err(|_| anyhow!("Wi-Fi join timed out"))??;
    ensure!(output.status.success(), "Wi-Fi join failed ({}); the previous network is kept", output.status);
    Ok(())
}

/// The marker in the camera's optical frame (x right, y down, z forward),
/// from its raw fisheye corner pixels.
fn marker_in_camera(c: &crate::camera::Calibration, corners_px: [[f64; 2]; 4]) -> Result<(Pose, f64)> {
    let eye = crate::fisheye::UnrectifiedEye {
        width: c.width, height: c.height, jpeg_base64: String::new(),
        k: c.k, d: c.d, r: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
    };
    eye.validate()?;
    let corners: Vec<[f64; 2]> = corners_px.iter().map(|p| eye.rectify(*p, c.k)).collect::<Option<_>>()
        .ok_or_else(|| anyhow!("Marker corners outside the lens model"))?;
    let gl = crate::square_pose::square_in_camera(c.k, [c.width as usize, c.height as usize],
        corners.try_into().unwrap(), ob::MARKER_SIDE_M)?;
    // The solver answers in the OpenGL camera frame; the head camera's frame is optical.
    let flip = Pose { translation: [0.; 3], rotation_xyzw: [1., 0., 0., 0.] };
    let range = gl.translation.iter().map(|v| v * v).sum::<f64>().sqrt();
    Ok((crate::alignment::compose(&flip, &gl)?, range))
}

/// One onboarding session, from an invitation to an agreed alignment.
async fn session(client: &ComponentProtocolClient, home: &crate::Home, invitation: &ob::Invitation,
                 codes: &mut tokio::sync::mpsc::UnboundedReceiver<Seen>,
                 session_clock: &auki_time::SessionClock, local_clock: &crate::clock_sync::RegisteredClock,
                 registry: &auki_protocols::registry::RegistryClient) -> Result<()> {
    ensure!(invitation.home.peer_id == home.peer_id && invitation.home.frame == home.frame,
        "Invitation is for another Home scene; restart the runner with that Home's profile");
    if let Some(wifi) = &invitation.wifi { join_wifi(wifi).await?; }
    // Name this robot in Home with the alias: bound to its Peer ID, as for a phone.
    let scene = home.frame.split(':').nth(1).ok_or_else(|| anyhow!("Home frame names no scene"))?;
    match Remote::connect(client.clone(), &home.peer_id, &home.routes).await {
        Ok(h) => { let _ = h.invoke::<_, bool>(&format!("home-phone-presence:{scene}"), "name", &serde_json::json!({"name": invitation.alias})).await; }
        Err(e) => crate::diagnostic_log!("Onboarding: could not name this robot in Home: {e}"),
    }
    let host = Remote::connect(client.clone(), &invitation.host.peer_id, &invitation.host.routes).await
        .context("Could not reach the phone showing the code")?;
    let session = invitation.session.clone();
    let marker: ob::Marker = host.invoke(ob::COMPONENT, "show", &ob::Show { session: session.clone() }).await?;
    ensure!(marker.session == session && marker.payload == invitation.marker_payload()
        && marker.side_length_m == ob::MARKER_SIDE_M && marker.scene_peer_id == home.peer_id
        && marker.scene_frame == home.frame, "The phone showed a different marker or scene");
    ensure!(local_clock.reference() == crate::clock_sync::reference(&session_clock.registry_entry()), "Local capture clock changed");
    let remote_clock = crate::clock_sync::resolve(client, registry, host.peer, host.route.clone(), &marker.clock).await?;
    let mut probes = Vec::new();
    for _ in 0..16 {
        let sent_ns = session_clock.now_ns();
        let reading: ob::ClockReading = host.invoke(ob::COMPONENT, "clock", &ob::ReadClock { session: session.clone(), clock: marker.clock.clone() }).await?;
        let received_ns = session_clock.now_ns();
        ensure!(reading.session == session && reading.clock == marker.clock, "Clock reading for another session or clock");
        probes.push(ob::ClockProbe { sent_ns, remote_ns: reading.now_ns, received_ns });
    }
    let clock = ob::ClockMapping::measure(local_clock, &remote_clock, &probes)?;
    ensure!(clock.uncertainty_ns <= ob::MAX_CLOCK_UNCERTAINTY_NS, "The phone answers too slowly to time it");
    crate::diagnostic_log!("Onboarding as {}: clock matched to ±{} ms; looking for the marker", invitation.alias, clock.uncertainty_ns / 1_000_000);
    *SHARED.agreed.lock().unwrap() = None;
    SHARED.sightings.lock().unwrap().clear();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let agreed = *SHARED.agreed.lock().unwrap();
        if let Some(at) = agreed {
            let _ = host.invoke::<_, bool>(ob::COMPONENT, "progress", &ob::Progress { session: session.clone(),
                stage: ob::Stage::Aligned, detail: format!("{} placed itself in the scene", invitation.alias) }).await;
            crate::diagnostic_log!("Onboarding: aligned from the marker at {at}");
            return Ok(());
        }
        let seen = tokio::select! {
            seen = codes.recv() => seen.ok_or_else(|| anyhow!("Camera stopped"))?,
            _ = tokio::time::sleep_until(deadline) => bail!("Onboarding timed out"),
            _ = tokio::time::sleep(Duration::from_millis(200)) => continue,
        };
        let Some((_, corners)) = seen.codes.iter().find(|(p, _)| *p == marker.payload) else { continue };
        let Some(calibration) = CALIBRATION.lock().unwrap().clone() else { continue };
        let attempt = async {
            let (in_camera, range_m) = marker_in_camera(&calibration, *corners)?;
            let request = ob::AskMarker { session: session.clone(), clock: clock.to_clock.clone(),
                timestamp_ns: clock.remote(&local_clock.reference(), seen.timestamp_ns)?, uncertainty_ns: clock.uncertainty_ns };
            let at: ob::MarkerAt = host.invoke(ob::COMPONENT, "marker_at", &request).await?;
            at.validate_answer(&request)?;
            let camera_in_home = crate::alignment::compose(&at.marker_to_scene, &crate::alignment::inverse(&in_camera)?)?;
            SHARED.sightings.lock().unwrap().push_back(Sighting { timestamp_ns: seen.timestamp_ns, camera_in_home,
                timing_error_m: ob::timing_error(&clock, &at, range_m), range_m });
            Ok::<(), anyhow::Error>(())
        };
        if let Err(e) = attempt.await { crate::diagnostic_log!("Onboarding sighting skipped: {e}"); }
    }
}

/// Run for the life of the runner: watch for invitations and onboard.
pub async fn run(client: ComponentProtocolClient, registry: auki_protocols::registry::RegistryClient,
    home: crate::Home, session_clock: auki_time::SessionClock, local_clock: crate::clock_sync::RegisteredClock) {
    let (tx, mut codes) = tokio::sync::mpsc::unbounded_channel();
    if CODES.set(tx).is_err() { return; }
    let mut seen = ob::Seen::default();
    while let Some(frame) = codes.recv().await {
        let Some(text) = frame.codes.iter().map(|(p, _)| p).find(|p| p.starts_with(ob::PREFIX)) else { continue };
        let invitation = match ob::Invitation::parse(text) {
            Ok(invitation) => invitation,
            Err(e) => { crate::diagnostic_log!("Onboarding code not usable: {e}"); continue; }
        };
        if !seen.first(&invitation.session) { continue; }
        crate::diagnostic_log!("Onboarding as {} started", invitation.alias);
        if let Err(e) = session(&client, &home, &invitation, &mut codes, &session_clock, &local_clock, &registry).await {
            crate::diagnostic_log!("Onboarding failed: {e}");
            // A failed session may be retried by showing the code again.
            seen.forget(&invitation.session);
        }
    }
}
