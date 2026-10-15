//! A phone's part in onboarding (onboarding.md): hosting, where this phone
//! shows the way into its scene, and joining, where it places itself in a
//! scene by watching another phone's marker. The arithmetic is in
//! `crate::onboarding`, shared with robots.
use crate::onboarding::{self as ob, Hosting, Stage};
use anyhow::{Result, anyhow, ensure};
use auki_components::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}};
use std::time::Duration;

/// State the host's operations and the app both read. `revision` moves
/// whenever something the app shows changes.
#[derive(Clone, Default)]
pub struct Shared {
    pub hosting: Arc<Mutex<Option<Hosting>>>,
    pub history: Arc<Mutex<ob::PoseHistory>>,
    pub joining: Arc<Mutex<Option<Joining>>>,
    pub revision: Arc<AtomicU64>,
}
impl Shared {
    pub fn touch(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }
    pub fn snapshot(&self) -> Option<serde_json::Value> {
        if let Some(hosting) = self.hosting.lock().ok()?.as_ref() {
            return Some(serde_json::json!({
                "role": "host", "session": hosting.session, "alias": hosting.alias,
                "invitation": hosting.invitation, "marker": hosting.marker,
                "joiner": hosting.joiner, "stage": hosting.stage, "detail": hosting.detail,
            }));
        }
        self.joining.lock().ok()?.as_ref().map(|j| serde_json::json!({
            "role": "joiner", "session": j.session, "alias": j.alias, "marker": j.marker,
            "looking": j.looking, "stage": j.stage, "detail": j.detail, "sightings": j.sightings,
            "clock_uncertainty_ms": j.clock.map(|c| c.uncertainty_ns as f64 / 1e6),
            "alignment": j.alignment,
        }))
    }
    fn host<R>(&self, f: impl FnOnce(&mut Hosting) -> Result<R>) -> Result<R> {
        let mut hosting = self.hosting.lock().map_err(|_| anyhow!("Onboarding unavailable"))?;
        let hosting = hosting.as_mut().ok_or_else(|| anyhow!("This phone is not onboarding anyone"))?;
        let result = f(hosting);
        self.touch();
        result
    }
    pub(crate) fn join(&self, f: impl FnOnce(&mut Joining)) {
        if let Ok(mut joining) = self.joining.lock() && let Some(joining) = joining.as_mut() {
            f(joining);
        }
        self.touch();
    }
}

/// The joiner's progress, as the app shows it.
#[derive(Clone, Serialize)]
pub struct Joining {
    pub session: String,
    pub alias: String,
    /// The marker to look for, once the host shows it.
    pub marker: Option<String>,
    /// True while the app should hand camera frames to `sighting`.
    pub looking: bool,
    pub stage: Option<Stage>,
    pub detail: String,
    pub sightings: usize,
    #[serde(skip)]
    pub clock: Option<ob::ClockMapping>,
    /// Where this phone's AR session sits in the scene, once agreed.
    pub alignment: Option<crate::mobile_depth::Alignment>,
}

/// One camera frame, as the joiner's app hands it over with its luma plane.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub session: String,
    pub timestamp_ns: u64,
    pub width: usize,
    pub height: usize,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub camera_to_session: auki_scenegraph::RigidTransform,
}
/// The marker in this phone's AR session at a capture time.
#[derive(Clone, Debug)]
pub struct Sighting {
    pub session: String,
    pub timestamp_ns: u64,
    pub marker_to_session: auki_odometry::Pose,
    pub range_m: f64,
}

/// Look for `marker` in a frame. Sync: the app calls it off the main thread.
pub fn sight(luma: &[u8], frame: &Frame, marker: &str) -> Result<Option<Sighting>> {
    uuid::Uuid::parse_str(&frame.session)?;
    ensure!(frame.camera_to_session.from_frame_id == format!("phone-camera:{}", frame.session)
        && frame.camera_to_session.to_frame_id == format!("phone-ar:{}", frame.session), "Wrong capture frame");
    let Some((_, corners)) = crate::mobile_calibration::decode(luma, frame.width, frame.height)?
        .into_iter().find(|(text, _)| text == marker) else { return Ok(None) };
    let in_camera = crate::mobile_calibration::square_in_camera(
        [frame.fx, frame.fy, frame.cx, frame.cy], [frame.width, frame.height], corners, ob::MARKER_SIDE_M)?;
    let camera = crate::local_mapper::from_rigid(&frame.camera_to_session);
    camera.validate().map_err(|e| anyhow!("Invalid AR pose: {e:?}"))?;
    let range_m = in_camera.translation.iter().map(|v| v * v).sum::<f64>().sqrt();
    Ok(Some(Sighting {
        session: frame.session.clone(),
        timestamp_ns: frame.timestamp_ns,
        marker_to_session: crate::local_mapper::compose(&camera, &in_camera)?,
        range_m,
    }))
}

/// The host's Component. Mounted on every connection; it refuses everything
/// until the app starts hosting, and then answers only the session's joiner.
pub struct Display {
    _component: Component,
}
impl Display {
    pub fn mount(
        products: &crate::mobile_products::MobileProducts,
        shared: &Shared,
        placement: tokio::sync::watch::Receiver<Option<crate::mobile_declared::Placement>>,
    ) -> Result<Self> {
        let component = products.runtime.component(ComponentSpec::new(ob::COMPONENT)
            .operable(ob::show_contract()).operable(ob::clock_contract())
            .operable(ob::marker_at_contract()).operable(ob::progress_contract()))?;
        let rejected = |e: anyhow::Error| InvocationError::Rejected(e.to_string());
        let authenticated = |ctx: &InvocationContext| !ctx.caller_peer_id.is_empty();

        let s = shared.clone();
        let show = component.operable("show", authenticated, move |ctx, request: ob::Show| {
            s.host(|h| {
                h.admit(&ctx.caller_peer_id, &request.session)?;
                h.stage = Some(Stage::Marker);
                h.detail = "Showing the marker".into();
                Ok(ob::Marker { session: h.session.clone(), payload: h.marker.clone(), side_length_m: ob::MARKER_SIDE_M,
                    scene_peer_id: h.scene_peer_id.clone(), scene_frame: h.scene_frame.clone() })
            }).map_err(rejected)
        })?;
        let s = shared.clone();
        let clock = component.operable("clock", authenticated, move |ctx, request: ob::ReadClock| {
            let session = s.host(|h| { h.answering(&ctx.caller_peer_id, &request.session)?; Ok(h.session.clone()) }).map_err(rejected)?;
            Ok(ob::ClockReading { session, now_ns: ob::capture_clock_now().map_err(rejected)? })
        })?;
        let s = shared.clone();
        let marker_at = component.operable("marker_at", authenticated, move |ctx, request: ob::AskMarker| {
            let marker_in_camera = s.host(|h| { h.answering(&ctx.caller_peer_id, &request.session)?; Ok(h.marker_in_camera.clone()) })
                .map_err(rejected)?;
            let placement = placement.borrow().clone()
                .ok_or_else(|| rejected(anyhow!("This phone lost its place in the scene")))?;
            let at = s.history.lock().map_err(|_| InvocationError::TargetUnavailable)?
                .at(&placement.session, request.timestamp_ns).map_err(rejected)?;
            let session_to_scene = crate::mobile_declared::pose(&placement.session_to_scene).map_err(rejected)?;
            Ok(ob::MarkerAt {
                session: request.session, timestamp_ns: request.timestamp_ns,
                marker_to_scene: ob::marker_in_scene(&session_to_scene, &at.pose, &marker_in_camera).map_err(rejected)?,
                speed_m_s: at.speed_m_s, turn_rad_s: at.turn_rad_s,
            })
        })?;
        let s = shared.clone();
        let progress = component.operable("progress", authenticated, move |ctx, report: ob::Progress| {
            s.host(|h| {
                h.answering(&ctx.caller_peer_id, &report.session)?;
                // Once aligned or failed, a late report cannot reopen the session.
                if !matches!(h.stage, Some(Stage::Aligned | Stage::Failed)) {
                    h.stage = Some(report.stage);
                    h.detail = report.detail.chars().filter(|c| !c.is_control()).take(200).collect();
                }
                Ok(true)
            }).map_err(rejected)
        })?;
        component.expose()?;
        for (name, result) in [
            ("show", products.endpoint.export_operable(&show)),
            ("clock", products.endpoint.export_operable(&clock)),
            ("marker_at", products.endpoint.export_operable(&marker_at)),
            ("progress", products.endpoint.export_operable(&progress)),
        ] {
            result.map_err(|e| anyhow!("Onboarding {name} not offered: {e}"))?;
        }
        Ok(Self { _component: component })
    }
}

/// Join the scene the invitation's host is placed in, through the host.
/// Returns when this phone has placed itself, or with why it could not.
pub async fn join(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
    invitation: &ob::Invitation,
    shared: &Shared,
    sightings: &mut tokio::sync::mpsc::Receiver<Sighting>,
) -> Result<crate::mobile_depth::Alignment> {
    ensure!(invitation.home.peer_id == home.peer_id && invitation.home.frame == home.frame,
        "The invitation is for another Home scene");
    let host = crate::mobile_home::HomeOperations::for_peer(peer, &invitation.host.peer_id, &invitation.host.routes, &home.frame)?;
    let session = invitation.session.clone();
    let say = |stage: Option<Stage>, detail: &str| shared.join(|j| { j.stage = stage; j.detail = detail.into(); });
    let call = |operation: &'static str| (ob::COMPONENT, operation, "phone-onboarding");
    let request = || uuid::Uuid::new_v4().to_string();

    say(None, "Reaching the phone showing the code");
    let (component, operation, caller) = call("show");
    let marker: ob::Marker = host.invoke(component, operation, caller, &request(), Duration::from_secs(4),
        &ob::Show { session: session.clone() }).await?;
    ensure!(marker.session == session && marker.payload == invitation.marker_payload()
        && marker.side_length_m == ob::MARKER_SIDE_M, "The host showed a different marker");
    ensure!(marker.scene_peer_id == home.peer_id && marker.scene_frame == home.frame,
        "The host is placed in another scene");

    say(Some(Stage::Marker), "Timing the other phone's clock");
    // Each exchange opens an authenticated stream, and a phone's Wi-Fi dozes
    // between packets, so the first few are slow. The tightest one bounds the
    // offset; the motion check per sighting decides whether it is good enough.
    let mut probes = Vec::new();
    for _ in 0..16 {
        let sent_ns = ob::capture_clock_now()?;
        let (component, operation, caller) = call("clock");
        let reading: ob::ClockReading = host.invoke(component, operation, caller, &request(), Duration::from_secs(2),
            &ob::ReadClock { session: session.clone() }).await?;
        let received_ns = ob::capture_clock_now()?;
        ensure!(reading.session == session, "Clock reading for another session");
        probes.push(ob::ClockProbe { sent_ns, remote_ns: reading.now_ns, received_ns });
    }
    let rtts: Vec<u64> = probes.iter().map(|p| p.received_ns.saturating_sub(p.sent_ns) / 1_000_000).collect();
    eprintln!("[onboarding] clock exchange round trips (ms): {rtts:?}");
    let clock = ob::ClockMapping::measure(&probes)?;
    ensure!(clock.uncertainty_ns <= ob::MAX_CLOCK_UNCERTAINTY_NS,
        "The other phone answers too slowly to time it (±{} ms); try again", clock.uncertainty_ns / 1_000_000);
    shared.join(|j| { j.clock = Some(clock); j.marker = Some(marker.payload.clone()); j.looking = true; });
    say(Some(Stage::Marker), &format!("Clock matched to ±{} ms · point this phone's camera at the other phone's screen",
        clock.uncertainty_ns / 1_000_000));

    while sightings.try_recv().is_ok() {}
    let mut agreement = ob::Agreement::default();
    let mut last_problem = String::new();
    let mut reported = 0;
    let agreed = loop {
        let sighting = sightings.recv().await.ok_or_else(|| anyhow!("Onboarding stopped"))?;
        let host_time = clock.remote(sighting.timestamp_ns)?;
        // The host may not have its pose for this instant yet: wait a little, never use a newer one.
        let mut answer = Err(anyhow!("No answer"));
        for attempt in 0..4 {
            if attempt > 0 { tokio::time::sleep(Duration::from_millis(60)).await; }
            let (component, operation, caller) = call("marker_at");
            answer = host.invoke::<_, ob::MarkerAt>(component, operation, caller, &request(), Duration::from_secs(2),
                &ob::AskMarker { session: session.clone(), timestamp_ns: host_time }).await;
            if !answer.as_ref().is_err_and(|e| e.to_string().contains("No pose yet")) { break; }
        }
        let at = match answer {
            Ok(at) if at.session == session && at.timestamp_ns == host_time => at,
            Ok(_) => return Err(anyhow!("The host answered for another moment")),
            Err(e) => { last_problem = e.to_string(); say(Some(Stage::Measuring), &last_problem); continue; }
        };
        let solution = ob::Solution {
            timestamp_ns: sighting.timestamp_ns,
            native_to_scene: ob::solve(&at.marker_to_scene, &sighting.marker_to_session)?,
            timing_error_m: ob::timing_error(&clock, &at, sighting.range_m),
            range_m: sighting.range_m,
        };
        match agreement.add(solution) {
            Ok(Some(agreed)) => break (sighting, agreed),
            Ok(None) => {}
            Err(e) => last_problem = e.to_string(),
        }
        shared.join(|j| j.sightings = agreement.len());
        let detail = if last_problem.is_empty() { format!("Seen {} times · hold both phones steady", agreement.len()) }
            else { std::mem::take(&mut last_problem) };
        say(Some(Stage::Measuring), &detail);
        if agreement.len() != reported {
            reported = agreement.len();
            let (component, operation, caller) = call("progress");
            let _ = host.invoke::<_, bool>(component, operation, caller, &request(), Duration::from_secs(1),
                &ob::Progress { session: session.clone(), stage: Stage::Measuring, detail: detail.clone() }).await;
        }
    };
    let (sighting, native_to_scene) = agreed;
    let ar = format!("phone-ar:{}", sighting.session);
    let alignment = crate::mobile_depth::Alignment {
        timestamp_ns: sighting.timestamp_ns,
        portal_id: marker.payload.clone(),
        portal_to_session: crate::local_mapper::rigid(&sighting.marker_to_session, &format!("onboarding-marker:{session}"), &ar),
        session_to_home: crate::local_mapper::rigid(&native_to_scene, &ar, &home.frame),
    };
    shared.join(|j| { j.looking = false; j.alignment = Some(alignment.clone()); j.stage = Some(Stage::Aligned);
        j.detail = "Placed in the scene".into(); });
    let (component, operation, caller) = call("progress");
    let _ = host.invoke::<_, bool>(component, operation, caller, &request(), Duration::from_secs(2),
        &ob::Progress { session: session.clone(), stage: Stage::Aligned, detail: format!("{} placed itself in the scene", invitation.alias) }).await;
    Ok(alignment)
}

/// A code's modules, row by row, quiet zone excluded, so the app can draw it
/// at an exact physical size. Medium error correction.
pub fn qr_modules(text: &str) -> Result<(usize, Vec<bool>)> {
    ensure!(text.len() <= 2048, "Too much text for a code");
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M)?;
    Ok((code.width(), code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onboarding::marker_payload;

    #[test]
    fn marker_is_short_alphanumeric_and_not_a_portal() {
        let payload = marker_payload("5f1c2a9e-3b7d-4c11-9a2e-0f6b7d8e9a10");
        assert_eq!(payload, "ONB:5F1C2A9E3B7D4C11");
        assert!(crate::local_mapper::portal_code(&payload).is_none());
        let code = qrcode::QrCode::with_error_correction_level(&payload, qrcode::EcLevel::M).unwrap();
        assert_eq!(code.width(), 21, "version 1 keeps the modules large");
        let (width, modules) = super::qr_modules(&payload).unwrap();
        assert_eq!((width, modules.len()), (21, 21 * 21));
        assert!(modules[0] && modules[20] && !modules[21 * 7], "finder corners dark, separator light");
    }


    #[test]
    fn snapshot_never_carries_the_invitation_for_a_joiner_and_shows_the_host_its_qr() {
        let shared = Shared::default();
        assert!(shared.snapshot().is_none());
        *shared.joining.lock().unwrap() = Some(Joining { session: "s".into(), alias: "Tilly".into(), marker: None,
            looking: false, stage: None, detail: String::new(), sightings: 0, clock: None, alignment: None });
        let before = shared.revision.load(Ordering::Acquire);
        shared.join(|j| j.detail = "x".into());
        assert!(shared.revision.load(Ordering::Acquire) > before);
        let snapshot = shared.snapshot().unwrap();
        assert_eq!(snapshot["role"], "joiner");
        assert_eq!(snapshot["detail"], "x");
    }
}
