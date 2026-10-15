//! Onboarding a peer into a Home scene from a phone that is already placed in it.
//! See onboarding.md. Everything here is shared: the phone that shows the way
//! (the host), and any joining peer, phone or robot.
//!
//! The host shows an invitation QR, then on request a small marker whose
//! physical size and place on its screen it knows. The joiner sees the marker,
//! asks the host where the marker was in the scene at that moment, and so
//! learns where its own native frame sits in the scene:
//!
//! ```text
//! T_H_M(t) = T_H_P × T_P_C(t) × T_C_M      host: scene ← AR session ← camera ← marker
//! T_R_M(t) = T_R_K(t) × T_K_M(t)           joiner: native ← camera ← marker
//! T_H_R    = T_H_M(t) × inverse(T_R_M(t))
//! ```
//!
//! The two peers capture on different clocks. The joiner measures the host's
//! clock against its own over the peer link and carries the uncertainty into
//! its acceptance test; it never pairs an observation with the latest pose.
use crate::local_mapper::{compose, inverse};
use anyhow::{Result, bail, ensure};
use auki_components::ContractType;
use auki_odometry::Pose;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const SCHEMA: &str = "spatial-harness.onboarding/v1";
/// The host's Component. Its operations answer only the peer that asked first
/// for the marker of the invitation it is showing.
pub const COMPONENT: &str = "onboarding-display";
/// The marker's encoded square, quiet zone excluded. 50 mm, not the 30 mm of
/// onboarding.md: a robot's wide fisheye cameras cannot resolve a 30 mm code.
pub const MARKER_SIDE_M: f64 = 0.05;
/// What an invitation QR starts with, so a scanner can tell one apart cheaply.
pub const PREFIX: &str = r#"{"schema":"spatial-harness.onboarding/v1""#;
const MAX_TEXT: usize = 2048;

/// What the invitation QR carries. Public connection details and, when the
/// joiner must change network, the Wi-Fi it needs. No peer credential: the
/// joiner still signs in with its own identity and the Domain decides.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invitation {
    pub schema: String,
    /// Pairing session: ties the QR, the marker, the operations and the result together.
    pub session: String,
    /// The name the joiner gives itself in Home. Bound to its Peer ID, never an identity.
    pub alias: String,
    pub home: crate::network::HomeConnection,
    pub host: Host,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi: Option<Wifi>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub peer_id: String,
    pub routes: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wifi {
    pub ssid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}
// Never print a Wi-Fi password, even in a debug log.
impl std::fmt::Debug for Wifi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wifi").field("ssid", &self.ssid).field("hidden", &self.hidden).finish_non_exhaustive()
    }
}
impl std::fmt::Debug for Invitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invitation").field("session", &self.session).field("alias", &self.alias)
            .field("host", &self.host.peer_id).finish_non_exhaustive()
    }
}

impl Invitation {
    pub fn new(alias: &str, home: crate::network::HomeConnection, host: Host, wifi: Option<Wifi>) -> Result<Self> {
        let invitation = Self {
            schema: SCHEMA.into(),
            session: uuid::Uuid::new_v4().to_string(),
            alias: alias.trim().into(),
            home, host, wifi,
        };
        invitation.validate()?;
        Ok(invitation)
    }
    /// Read a scanned QR. Bounded and typed; anything else is not an invitation.
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(text.len() <= MAX_TEXT, "Onboarding QR is too large");
        ensure!(text.starts_with(PREFIX), "Not an onboarding QR");
        let invitation: Self = serde_json::from_str(text)?;
        invitation.validate()?;
        Ok(invitation)
    }
    /// The QR text. The schema comes first, so a scanner can test `PREFIX`.
    pub fn encode(&self) -> Result<String> {
        self.validate()?;
        let text = serde_json::to_string(self)?;
        ensure!(text.starts_with(PREFIX) && text.len() <= MAX_TEXT, "Onboarding QR would be too large");
        Ok(text)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == SCHEMA, "Unknown onboarding schema");
        uuid::Uuid::parse_str(&self.session)?;
        let alias = self.alias.trim();
        ensure!(!alias.is_empty() && alias.chars().count() <= 64 && !alias.chars().any(char::is_control),
            "Alias must be 1–64 printable characters");
        ensure!(self.home.schema == "spatial-harness.home-connection/v1", "Unknown Home profile");
        self.home.validate_scene_contract()?;
        ensure!(!self.home.routes.is_empty() && self.home.routes.len() <= 4, "Home needs 1–4 routes in an invitation");
        let _: auki_sdk::PeerId = self.home.peer_id.parse()?;
        let _: auki_sdk::PeerId = self.host.peer_id.parse()?;
        ensure!(self.host.peer_id != self.home.peer_id, "The host is not Home");
        ensure!(!self.host.routes.is_empty() && self.host.routes.len() <= 4, "Host needs 1–4 routes");
        for route in self.home.routes.iter().chain(&self.host.routes) {
            let _: auki_sdk::Multiaddr = route.parse()?;
        }
        if let Some(wifi) = &self.wifi {
            ensure!(!wifi.ssid.is_empty() && wifi.ssid.len() <= 32, "Wi-Fi name must be 1–32 bytes");
            if let Some(password) = &wifi.password {
                ensure!((8..=63).contains(&password.len()), "Wi-Fi password must be 8–63 characters");
            }
        }
        Ok(())
    }
    /// The marker shown for this session. Not a Portal: no Portal reader takes
    /// it for one, so it never becomes part of a scene.
    pub fn marker_payload(&self) -> String {
        marker_payload(&self.session)
    }
}

/// `ONB:` and the session's first 64 bits: QR alphanumeric, version 1.
pub fn marker_payload(session: &str) -> String {
    let hex: String = session.chars().filter(|c| c.is_ascii_hexdigit()).take(16).collect();
    format!("ONB:{}", hex.to_ascii_uppercase())
}

/// A code's modules, row by row, quiet zone excluded, so the app can draw it
/// at an exact physical size. Medium error correction.
pub fn qr_modules(text: &str) -> Result<(usize, Vec<bool>)> {
    ensure!(text.len() <= MAX_TEXT, "Too much text for a code");
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M)?;
    Ok((code.width(), code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect()))
}

/// Scans seen in normal operation, so a QR held in view does not start the
/// same session again. A failed session is retried only when asked.
#[derive(Default)]
pub struct Seen(VecDeque<String>);
impl Seen {
    /// True the first time a session is seen.
    pub fn first(&mut self, session: &str) -> bool {
        if self.0.iter().any(|s| s == session) {
            return false;
        }
        if self.0.len() == 16 {
            self.0.pop_front();
        }
        self.0.push_back(session.into());
        true
    }
    pub fn forget(&mut self, session: &str) {
        self.0.retain(|s| s != session);
    }
}

// ---------------------------------------------------------------------------
// The host's screen

/// A phone model whose screen and camera we have numbers for. Lengths in
/// millimetres, in the phone's front view held upright: x right, y up, z out
/// of the screen, origin at the centre of the screen.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Screen {
    pub model: &'static str,
    pub name: &'static str,
    /// Physical size of one UIKit point.
    pub mm_per_point: f64,
    /// The screen in points, portrait.
    pub points: [f64; 2],
    /// Optical centre of the camera AR tracking uses (the main wide camera).
    pub camera_mm: [f64; 3],
    /// False until a ruler and an alignment check have confirmed the numbers.
    /// Sizes come from Apple's published resolution and pixel density; the
    /// camera position is read off published drawings and may be a few mm out.
    pub verified: bool,
}
const fn mm_per_point(scale: f64, ppi: f64) -> f64 {
    scale / ppi * 25.4
}
pub const SCREENS: &[Screen] = &[
    Screen { model: "iPhone18,2", name: "iPhone 17 Pro Max", mm_per_point: mm_per_point(3., 460.),
        points: [440., 956.], camera_mm: [25., 67.5, -10.], verified: false },
    Screen { model: "iPhone18,1", name: "iPhone 17 Pro", mm_per_point: mm_per_point(3., 460.),
        points: [402., 874.], camera_mm: [22.5, 61., -10.], verified: false },
    Screen { model: "iPhone16,1", name: "iPhone 15 Pro", mm_per_point: mm_per_point(3., 460.),
        points: [393., 852.], camera_mm: [22.3, 60.3, -10.], verified: false },
    Screen { model: "iPhone14,3", name: "iPhone 13 Pro Max", mm_per_point: mm_per_point(3., 458.),
        points: [428., 926.], camera_mm: [26., 67.4, -10.], verified: false },
];
pub fn screen(model: &str) -> Result<&'static Screen> {
    SCREENS.iter().find(|s| s.model == model)
        .ok_or_else(|| anyhow::anyhow!("No screen measurements for {model}; this phone cannot show an onboarding marker"))
}

/// Where the app drew the marker, in portrait points from the screen's top left.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display {
    pub model: String,
    pub center_points: [f64; 2],
    pub side_points: f64,
}
impl Display {
    /// The marker in the AR camera frame, `T_C_M`. The camera frame is ARKit's:
    /// x toward the bottom of the phone, y toward its right edge, z out of the
    /// screen. The marker frame is the Portal solver's: x right, y up as the
    /// code reads, z out of its face. The app shows the marker upright in portrait.
    pub fn marker_in_camera(&self) -> Result<Pose> {
        let screen = screen(&self.model)?;
        let [x, y] = self.center_points;
        ensure!(x.is_finite() && y.is_finite() && self.side_points.is_finite(), "Invalid marker layout");
        let half = self.side_points / 2.;
        ensure!(x - half >= 0. && y - half >= 0. && x + half <= screen.points[0] && y + half <= screen.points[1],
            "Marker is not wholly on the screen");
        let side_mm = self.side_points * screen.mm_per_point;
        ensure!((side_mm - MARKER_SIDE_M * 1000.).abs() <= 0.25, "Marker is {side_mm:.2} mm, not {} mm", MARKER_SIDE_M * 1000.);
        let k = screen.mm_per_point;
        let marker = [(x - screen.points[0] / 2.) * k, (screen.points[1] / 2. - y) * k, 0.];
        let d: [f64; 3] = std::array::from_fn(|i| (marker[i] - screen.camera_mm[i]) / 1000.);
        let s = std::f64::consts::FRAC_1_SQRT_2;
        Ok(Pose { translation: [-d[1], d[0], d[2]], rotation_xyzw: [0., 0., s, s] })
    }
}

// ---------------------------------------------------------------------------
// Clocks

/// One exchange: the joiner's clock when it asked and when the answer came,
/// and the host's clock as it answered.
#[derive(Clone, Copy, Debug)]
pub struct ClockProbe {
    pub sent_ns: u64,
    pub remote_ns: u64,
    pub received_ns: u64,
}
/// `remote = local + offset`, good to `± uncertainty`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClockMapping {
    pub offset_ns: i64,
    pub uncertainty_ns: u64,
}
impl ClockMapping {
    /// The tightest exchange bounds the offset best: the host answered
    /// somewhere between ask and answer.
    pub fn measure(probes: &[ClockProbe]) -> Result<Self> {
        ensure!(probes.len() >= 3, "Too few clock exchanges");
        let best = probes.iter()
            .filter(|p| p.received_ns >= p.sent_ns && p.remote_ns > 0)
            .min_by_key(|p| p.received_ns - p.sent_ns)
            .ok_or_else(|| anyhow::anyhow!("No usable clock exchange"))?;
        let rtt = best.received_ns - best.sent_ns;
        let middle = best.sent_ns + rtt / 2;
        Ok(Self { offset_ns: best.remote_ns as i64 - middle as i64, uncertainty_ns: rtt / 2 + 1 })
    }
    pub fn remote(&self, local_ns: u64) -> Result<u64> {
        let t = local_ns as i128 + self.offset_ns as i128;
        ensure!(t > 0 && t < u64::MAX as i128, "Clock mapping out of range");
        Ok(t as u64)
    }
}

/// The phone's AR capture clock now: the clock ARKit stamps frames with
/// (seconds since boot, not counting sleep).
pub fn capture_clock_now() -> Result<u64> {
    #[cfg(target_vendor = "apple")]
    {
        unsafe extern "C" {
            fn clock_gettime_nsec_np(clock: u32) -> u64;
        }
        const CLOCK_UPTIME_RAW: u32 = 8;
        let now = unsafe { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) };
        ensure!(now > 0, "Capture clock unavailable");
        Ok(now)
    }
    #[cfg(not(target_vendor = "apple"))]
    bail!("This device's capture clock is not known to onboarding yet")
}

// ---------------------------------------------------------------------------
// The host's pose history

/// Recent camera poses of one AR session, for answering "where was the marker
/// at time t". Interpolates between neighbours a short gap apart; never
/// extrapolates and never answers with the nearest pose.
pub struct PoseHistory {
    session: String,
    samples: VecDeque<(u64, Pose)>,
}
const HISTORY: usize = 600;
const MAX_GAP_NS: u64 = 120_000_000;
impl Default for PoseHistory {
    fn default() -> Self { Self { session: String::new(), samples: VecDeque::with_capacity(HISTORY) } }
}
#[derive(Clone, Debug)]
pub struct PoseAt {
    pub pose: Pose,
    /// How fast the camera was moving, for the joiner's error budget.
    pub speed_m_s: f64,
    pub turn_rad_s: f64,
}
impl PoseHistory {
    pub fn session(&self) -> &str { &self.session }
    pub fn push(&mut self, session: &str, timestamp_ns: u64, pose: Pose) {
        if self.session != session {
            self.session = session.into();
            self.samples.clear();
        }
        if self.samples.back().is_some_and(|(t, _)| *t >= timestamp_ns) {
            return;
        }
        if self.samples.len() == HISTORY {
            self.samples.pop_front();
        }
        self.samples.push_back((timestamp_ns, pose));
    }
    /// Tracking was lost: nothing before now may be bridged to what follows.
    pub fn lose(&mut self) {
        self.samples.clear();
    }
    pub fn at(&self, session: &str, t: u64) -> Result<PoseAt> {
        ensure!(self.session == session, "Pose history is for another AR session");
        let after = self.samples.iter().position(|(at, _)| *at >= t)
            .ok_or_else(|| anyhow::anyhow!("No pose yet at that time"))?;
        let (t1, p1) = &self.samples[after];
        if *t1 == t {
            let neighbour = after.checked_sub(1).or((after + 1 < self.samples.len()).then_some(after + 1))
                .ok_or_else(|| anyhow::anyhow!("Too few poses"))?;
            let (tn, pn) = &self.samples[neighbour];
            let (speed, turn) = motion(p1, pn, t1.abs_diff(*tn));
            return Ok(PoseAt { pose: p1.clone(), speed_m_s: speed, turn_rad_s: turn });
        }
        let before = after.checked_sub(1).ok_or_else(|| anyhow::anyhow!("That time is older than the pose history"))?;
        let (t0, p0) = &self.samples[before];
        ensure!(t1 - t0 <= MAX_GAP_NS, "Poses around that time are too far apart");
        let f = (t - t0) as f64 / (t1 - t0) as f64;
        let (speed, turn) = motion(p0, p1, t1 - t0);
        Ok(PoseAt { pose: interpolate(p0, p1, f), speed_m_s: speed, turn_rad_s: turn })
    }
}
fn motion(a: &Pose, b: &Pose, dt_ns: u64) -> (f64, f64) {
    let dt = (dt_ns.max(1)) as f64 / 1e9;
    let d = distance(a, b);
    (d.0 / dt, d.1 / dt)
}
/// Translation distance and rotation angle between two poses.
pub fn distance(a: &Pose, b: &Pose) -> (f64, f64) {
    let t = (0..3).map(|i| (a.translation[i] - b.translation[i]).powi(2)).sum::<f64>().sqrt();
    let dot = (0..4).map(|i| a.rotation_xyzw[i] * b.rotation_xyzw[i]).sum::<f64>().abs().min(1.);
    (t, 2. * dot.acos())
}
pub fn interpolate(a: &Pose, b: &Pose, f: f64) -> Pose {
    let translation = std::array::from_fn(|i| a.translation[i] + (b.translation[i] - a.translation[i]) * f);
    let (qa, mut qb) = (a.rotation_xyzw, b.rotation_xyzw);
    let mut dot: f64 = (0..4).map(|i| qa[i] * qb[i]).sum();
    if dot < 0. {
        qb = qb.map(|v| -v);
        dot = -dot;
    }
    let q: [f64; 4] = if dot > 0.9995 {
        std::array::from_fn(|i| qa[i] + (qb[i] - qa[i]) * f)
    } else {
        let theta = dot.acos();
        let (wa, wb) = (((1. - f) * theta).sin() / theta.sin(), (f * theta).sin() / theta.sin());
        std::array::from_fn(|i| qa[i] * wa + qb[i] * wb)
    };
    let n = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    Pose { translation, rotation_xyzw: q.map(|v| v / n) }
}

// ---------------------------------------------------------------------------
// What the host and the joiner say to each other

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Show { pub session: String }
impl ContractType for Show { const DATATYPE: &'static str = "spatial-harness.onboarding-show/v1"; }

/// The marker the host now shows, and the scene its answers are in.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub session: String,
    pub payload: String,
    pub side_length_m: f64,
    /// The scene frame `marker_at` answers in: Home's peer and frame ID.
    pub scene_peer_id: String,
    pub scene_frame: String,
}
impl ContractType for Marker { const DATATYPE: &'static str = "spatial-harness.onboarding-marker/v1"; }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadClock { pub session: String }
impl ContractType for ReadClock { const DATATYPE: &'static str = "spatial-harness.onboarding-read-clock/v1"; }
/// The host's AR capture clock as it answered.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockReading { pub session: String, pub now_ns: u64 }
impl ContractType for ClockReading { const DATATYPE: &'static str = "spatial-harness.onboarding-clock/v1"; }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskMarker { pub session: String, pub timestamp_ns: u64 }
impl ContractType for AskMarker { const DATATYPE: &'static str = "spatial-harness.onboarding-ask-marker/v1"; }
/// The marker in the scene at the host's capture time asked for.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerAt {
    pub session: String,
    pub timestamp_ns: u64,
    pub marker_to_scene: Pose,
    pub speed_m_s: f64,
    pub turn_rad_s: f64,
}
impl ContractType for MarkerAt { const DATATYPE: &'static str = "spatial-harness.onboarding-marker-at/v1"; }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// The joiner is on the network and asked for the marker.
    Marker,
    /// It sees the marker and is matching sightings with the host's poses.
    Measuring,
    /// It placed itself in the scene and published that.
    Aligned,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress { pub session: String, pub stage: Stage, pub detail: String }
impl ContractType for Progress { const DATATYPE: &'static str = "spatial-harness.onboarding-progress/v1"; }

pub fn show_contract() -> auki_components::OperableContract { contract("show", Show::DATATYPE, Marker::DATATYPE) }
pub fn clock_contract() -> auki_components::OperableContract { contract("clock", ReadClock::DATATYPE, ClockReading::DATATYPE) }
pub fn marker_at_contract() -> auki_components::OperableContract { contract("marker_at", AskMarker::DATATYPE, MarkerAt::DATATYPE) }
pub fn progress_contract() -> auki_components::OperableContract { contract("progress", Progress::DATATYPE, "bool") }
fn contract(name: &str, instruction: &str, result: &str) -> auki_components::OperableContract {
    auki_components::OperableContract {
        name: name.into(), instruction: instruction.into(), result: result.into(),
        exposure: auki_components::Exposure::Cluster,
    }
}

/// The host's side of one session: who it answers, and what it last heard.
#[derive(Clone, Debug, Serialize)]
pub struct Hosting {
    pub session: String,
    pub alias: String,
    /// The invitation QR text. Holds the Wi-Fi password when there is one.
    #[serde(skip)]
    pub invitation: String,
    pub marker: String,
    #[serde(skip)]
    pub marker_in_camera: Pose,
    /// The scene the host is placed in, which its answers are in.
    #[serde(skip)]
    pub scene_peer_id: String,
    #[serde(skip)]
    pub scene_frame: String,
    /// The peer that asked first. Nobody else is answered.
    pub joiner: Option<String>,
    pub stage: Option<Stage>,
    pub detail: String,
}
impl Hosting {
    /// The first authenticated caller binds the session; later ones are refused.
    pub fn admit(&mut self, caller: &str, session: &str) -> Result<()> {
        ensure!(session == self.session, "Not the session this phone is showing");
        match &self.joiner {
            Some(joiner) => ensure!(joiner == caller, "Another device is onboarding"),
            None => self.joiner = Some(caller.into()),
        }
        Ok(())
    }
    pub fn answering(&self, caller: &str, session: &str) -> Result<()> {
        ensure!(session == self.session && self.joiner.as_deref() == Some(caller), "Not this session's device");
        Ok(())
    }
}

/// `T_H_M(t)`: the host's marker in the scene at its capture time `t`.
pub fn marker_in_scene(session_to_scene: &Pose, camera_to_session: &Pose, marker_in_camera: &Pose) -> Result<Pose> {
    compose(session_to_scene, &compose(camera_to_session, marker_in_camera)?)
}

// ---------------------------------------------------------------------------
// The joiner's solution

/// One sighting of the marker by the joiner, matched with where the host says
/// it was then.
#[derive(Clone, Debug)]
pub struct Solution {
    pub timestamp_ns: u64,
    /// `T_H_R`: the joiner's native frame in the scene.
    pub native_to_scene: Pose,
    /// How far the clock uncertainty alone could have moved the marker.
    pub timing_error_m: f64,
    pub range_m: f64,
}
pub fn solve(marker_to_scene: &Pose, marker_to_native: &Pose) -> Result<Pose> {
    compose(marker_to_scene, &inverse(marker_to_native)?)
}

/// Worst-case position error at the marker's distance from the error in the
/// time pairing: translation plus the lever of the host's turning.
pub fn timing_error(clock: &ClockMapping, at: &MarkerAt, range_m: f64) -> f64 {
    let dt = clock.uncertainty_ns as f64 / 1e9;
    dt * (at.speed_m_s + at.turn_rad_s * range_m.max(0.1))
}

/// Accepts the joiner's frame once enough sightings agree.
pub struct Agreement {
    solutions: Vec<Solution>,
}
pub const AGREE_M: f64 = 0.02;
pub const AGREE_RAD: f64 = 2. * std::f64::consts::PI / 180.;
pub const MAX_TIMING_ERROR_M: f64 = 0.01;
/// Beyond this the clock match is broken, not just loose. Within it, each
/// sighting is still refused when the host moved enough in that time to
/// shift the marker by more than `MAX_TIMING_ERROR_M`.
pub const MAX_CLOCK_UNCERTAINTY_NS: u64 = 400_000_000;
const NEEDED: usize = 5;
impl Default for Agreement { fn default() -> Self { Self { solutions: Vec::new() } } }
impl Agreement {
    pub fn len(&self) -> usize { self.solutions.len() }
    pub fn is_empty(&self) -> bool { self.solutions.is_empty() }
    /// Add a sighting. Returns the agreed frame once `NEEDED` recent
    /// sightings lie within `AGREE_M` and `AGREE_RAD` of their middle one.
    pub fn add(&mut self, solution: Solution) -> Result<Option<Pose>> {
        if solution.timing_error_m > MAX_TIMING_ERROR_M {
            bail!("The host phone moved too fast to time the sighting; hold it still");
        }
        self.solutions.push(solution);
        if self.solutions.len() > 12 {
            self.solutions.remove(0);
        }
        if self.solutions.len() < NEEDED {
            return Ok(None);
        }
        // The sighting most others agree with, and those that agree with it.
        let best = self.solutions.iter().map(|a| {
            self.solutions.iter().filter(|b| {
                let (t, r) = distance(&a.native_to_scene, &b.native_to_scene);
                t <= AGREE_M && r <= AGREE_RAD
            }).count()
        }).enumerate().max_by_key(|(_, n)| *n).map(|(i, _)| i).unwrap();
        let centre = self.solutions[best].native_to_scene.clone();
        let agreeing: Vec<&Pose> = self.solutions.iter().map(|s| &s.native_to_scene).filter(|p| {
            let (t, r) = distance(&centre, p);
            t <= AGREE_M && r <= AGREE_RAD
        }).collect();
        if agreeing.len() < NEEDED {
            return Ok(None);
        }
        Ok(Some(mean(&agreeing)))
    }
}
fn mean(poses: &[&Pose]) -> Pose {
    let n = poses.len() as f64;
    let translation = std::array::from_fn(|i| poses.iter().map(|p| p.translation[i]).sum::<f64>() / n);
    let first = poses[0].rotation_xyzw;
    let mut q = [0.; 4];
    for p in poses {
        let same = (0..4).map(|i| first[i] * p.rotation_xyzw[i]).sum::<f64>() >= 0.;
        for i in 0..4 {
            q[i] += if same { p.rotation_xyzw[i] } else { -p.rotation_xyzw[i] };
        }
    }
    let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    Pose { translation, rotation_xyzw: q.map(|v| v / norm) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> crate::network::HomeConnection {
        crate::network::HomeConnection {
            schema: "spatial-harness.home-connection/v1".into(),
            peer_id: "12D3KooWEyoppNCUx8Yx66oV9fJnriXwCcXwDDUA2kj6vnc6iDEp".into(),
            domain_id: "d3e1d6a6-0000-4000-8000-000000000001".into(),
            routes: vec!["/ip4/192.168.1.186/tcp/4001".into()],
            frame: "local-scene:hub:portal-HNY3JGNLRAE".into(),
            product_id: crate::local_scenes::mobile_product("hub"),
        }
    }
    fn host() -> Host {
        Host { peer_id: "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN".into(),
            routes: vec!["/ip4/192.168.1.20/tcp/50123".into()] }
    }
    fn pose(t: [f64; 3], q: [f64; 4]) -> Pose { Pose { translation: t, rotation_xyzw: q } }
    fn close(a: &Pose, b: &Pose) -> bool { let (t, r) = distance(a, b); t < 1e-9 && r < 1e-6 }

    #[test]
    fn invitation_round_trips_and_starts_with_its_schema() {
        let wifi = Wifi { ssid: "Lab".into(), password: Some("correct horse".into()), hidden: false };
        let invitation = Invitation::new(" Tilly ", home(), host(), Some(wifi)).unwrap();
        assert_eq!(invitation.alias, "Tilly");
        let text = invitation.encode().unwrap();
        assert!(text.starts_with(PREFIX), "{text}");
        let read = Invitation::parse(&text).unwrap();
        assert_eq!(read.session, invitation.session);
        assert_eq!(read.wifi.unwrap().password.as_deref(), Some("correct horse"));
        assert!(!format!("{invitation:?}").contains("correct horse"));
    }

    #[test]
    fn rejects_what_is_not_a_well_formed_invitation() {
        let good = Invitation::new("Tilly", home(), host(), None).unwrap();
        assert!(Invitation::parse("HTTPS://R8.HR/HNY3JGNLRAE").is_err());
        let text = good.encode().unwrap();
        assert!(Invitation::parse(&text.replace("\"Tilly\"", "\"\"")).is_err());
        assert!(Invitation::parse(&text.replace("}", ",\"run\":\"rm -rf /\"}")).is_err());
        let mut same = good.clone();
        same.host.peer_id = same.home.peer_id.clone();
        assert!(same.encode().is_err());
        let mut short = good.clone();
        short.wifi = Some(Wifi { ssid: "Lab".into(), password: Some("short".into()), hidden: false });
        assert!(short.encode().is_err());
        assert!(Invitation::parse(&format!("{text}{}", " ".repeat(MAX_TEXT))).is_err());
    }

    #[test]
    fn marker_is_short_alphanumeric_and_not_a_portal() {
        let payload = marker_payload("5f1c2a9e-3b7d-4c11-9a2e-0f6b7d8e9a10");
        assert_eq!(payload, "ONB:5F1C2A9E3B7D4C11");
        assert!(crate::local_mapper::portal_code(&payload).is_none());
        let code = qrcode::QrCode::with_error_correction_level(&payload, qrcode::EcLevel::M).unwrap();
        assert_eq!(code.width(), 21, "version 1 keeps the modules large");
        let (width, modules) = qr_modules(&payload).unwrap();
        assert_eq!((width, modules.len()), (21, 21 * 21));
        assert!(modules[0] && modules[20] && !modules[21 * 7], "finder corners dark, separator light");
    }

    #[test]
    fn scans_of_one_session_start_it_once() {
        let mut seen = Seen::default();
        assert!(seen.first("a"));
        assert!(!seen.first("a"));
        seen.forget("a");
        assert!(seen.first("a"));
    }

    #[test]
    fn marker_sits_where_the_screen_shows_it_relative_to_the_camera() {
        let screen = screen("iPhone16,1").unwrap();
        let side = MARKER_SIDE_M * 1000. / screen.mm_per_point;
        let centre = Display { model: "iPhone16,1".into(), center_points: [screen.points[0] / 2., screen.points[1] / 2.], side_points: side };
        let m = centre.marker_in_camera().unwrap();
        // Screen centre from the camera: left by 22.3, down by 60.3, out by 10 mm.
        // Camera x points to the phone's bottom and y to its right edge.
        let expected = [0.0603, -0.0223, 0.010];
        for i in 0..3 { assert!((m.translation[i] - expected[i]).abs() < 1e-9, "{:?}", m.translation); }
        // Marker x (right in portrait) is camera y; marker y (up) is camera -x; both face the user.
        let r = |v: [f64; 3]| compose(&m, &pose(v, [0., 0., 0., 1.])).unwrap().translation;
        let o = m.translation;
        let x = r([1., 0., 0.]);
        let y = r([0., 1., 0.]);
        assert!(((x[0] - o[0]).abs() < 1e-9) && ((x[1] - o[1]) - 1.).abs() < 1e-9);
        assert!(((y[0] - o[0]) + 1.).abs() < 1e-9 && ((y[1] - o[1]).abs() < 1e-9));
        let mut wrong = centre.clone();
        wrong.side_points = (MARKER_SIDE_M * 1000. + 1.) / screen.mm_per_point;
        assert!(wrong.marker_in_camera().is_err());
        let mut off = centre.clone();
        off.center_points = [5., 5.];
        assert!(off.marker_in_camera().is_err());
        assert!(Display { model: "iPhone1,1".into(), ..centre }.marker_in_camera().is_err());
    }

    #[test]
    fn clock_offset_comes_from_the_tightest_exchange() {
        let probes = [
            ClockProbe { sent_ns: 1_000, remote_ns: 51_000_000, received_ns: 40_001_000 },
            ClockProbe { sent_ns: 100_000_000, remote_ns: 150_000_000 + 5_000_000, received_ns: 110_000_000 },
            ClockProbe { sent_ns: 200_000_000, remote_ns: 300_000_000, received_ns: 260_000_000 },
        ];
        let mapping = ClockMapping::measure(&probes).unwrap();
        assert_eq!(mapping.offset_ns, 50_000_000);
        assert_eq!(mapping.uncertainty_ns, 5_000_001);
        assert_eq!(mapping.remote(1_000_000_000).unwrap(), 1_050_000_000);
        assert!(ClockMapping::measure(&probes[..2]).is_err());
    }

    #[test]
    fn history_interpolates_between_neighbours_and_never_beyond() {
        let mut history = PoseHistory::default();
        let s = "session";
        history.push(s, 1_000_000_000, pose([0., 0., 0.], [0., 0., 0., 1.]));
        let half = std::f64::consts::FRAC_1_SQRT_2;
        history.push(s, 1_100_000_000, pose([1., 0., 0.], [0., half, 0., half]));
        let mid = history.at(s, 1_050_000_000).unwrap();
        assert!((mid.pose.translation[0] - 0.5).abs() < 1e-9);
        let (_, angle) = distance(&mid.pose, &pose([0., 0., 0.], [0., 0., 0., 1.]));
        assert!((angle - std::f64::consts::FRAC_PI_4).abs() < 1e-9);
        assert!((mid.speed_m_s - 10.).abs() < 1e-9);
        assert!(history.at(s, 1_200_000_000).is_err(), "no extrapolation");
        assert!(history.at(s, 900_000_000).is_err());
        assert!(history.at("other", 1_050_000_000).is_err());
        history.push(s, 1_400_000_000, pose([1., 0., 0.], [0., 0., 0., 1.]));
        assert!(history.at(s, 1_300_000_000).is_err(), "gap too long");
        history.lose();
        assert!(history.at(s, 1_050_000_000).is_err());
    }

    #[test]
    fn the_joiner_lands_where_the_marker_says() {
        // The joiner's native frame sits 2 m along x in the scene, turned 90° about y.
        let half = std::f64::consts::FRAC_1_SQRT_2;
        let truth = pose([2., 0., 0.], [0., half, 0., half]);
        let host_session = pose([0.5, 1.0, -0.3], [0., 0., 0., 1.]);
        let host_camera = pose([0.1, 0.2, 0.3], [0.1, 0.2, 0.3, 0.927_361_849_549_570_4]);
        let host_camera = Pose { rotation_xyzw: { let q = host_camera.rotation_xyzw; let n = q.iter().map(|v| v * v).sum::<f64>().sqrt(); q.map(|v| v / n) }, ..host_camera };
        let display = Display { model: "iPhone18,2".into(), center_points: [220., 478.], side_points: MARKER_SIDE_M * 1000. / screen("iPhone18,2").unwrap().mm_per_point };
        let marker = marker_in_scene(&host_session, &host_camera, &display.marker_in_camera().unwrap()).unwrap();
        // What the joiner would measure: the same marker in its own frame.
        let seen = compose(&inverse(&truth).unwrap(), &marker).unwrap();
        assert!(close(&solve(&marker, &seen).unwrap(), &truth));
    }

    #[test]
    fn agreement_needs_several_close_sightings_and_steady_timing() {
        let at = |x: f64| Solution { timestamp_ns: 1, native_to_scene: pose([x, 0., 0.], [0., 0., 0., 1.]), timing_error_m: 0.001, range_m: 0.3 };
        let mut agreement = Agreement::default();
        assert!(agreement.add(Solution { timing_error_m: 0.05, ..at(0.) }).is_err());
        for x in [0., 0.5, 0.004, -0.003] {
            assert!(agreement.add(at(x)).unwrap().is_none());
        }
        assert!(agreement.add(at(0.002)).unwrap().is_none(), "the outlier does not count");
        let agreed = agreement.add(at(0.001)).unwrap().unwrap();
        assert!((agreed.translation[0] - 0.0008).abs() < 1e-9, "{:?}", agreed.translation);
    }

    #[test]
    fn hosting_answers_only_the_first_caller() {
        let mut hosting = Hosting { session: "s".into(), alias: "a".into(), invitation: String::new(), marker: String::new(),
            marker_in_camera: pose([0.; 3], [0., 0., 0., 1.]), scene_peer_id: String::new(), scene_frame: String::new(), joiner: None, stage: None, detail: String::new() };
        assert!(hosting.answering("peer", "s").is_err());
        hosting.admit("peer", "s").unwrap();
        hosting.admit("peer", "s").unwrap();
        assert!(hosting.admit("other", "s").is_err());
        assert!(hosting.admit("peer", "t").is_err());
        hosting.answering("peer", "s").unwrap();
        assert!(hosting.answering("other", "s").is_err());
    }
}
