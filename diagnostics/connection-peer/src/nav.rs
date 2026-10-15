//! `navigate` and `cancel` over the BBOS nav app, through a local Python
//! adapter on stdin/stdout like the arm and camera adapters. The adapter
//! declares where nav's frame sits relative to the SLAM world (nav's own
//! `T_nav_gl` rule); this peer publishes that as a held transform so goals reach
//! nav only through declared transforms. Only the robot's Home peer may call.
use crate::compact_pose::{Contract, FrameRef, Lifetime, Update};
use crate::navigate::*;
use crate::pointer::{Frames, Transform, chain, now_ns, through};
use anyhow::{Context, Result, anyhow, ensure};
use auki_components::*;
use auki_protocols::registry::v3::RegistryKind;
use auki_registry::{CoordinateConvention, FrameRegistryEntry};
use serde::Deserialize;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

pub const MOUNT: &str = "bbos-nav-frame";
const REPLY_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    nav_from_world: auki_odometry::Pose,
    mode: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Reply {
    request_id: Option<String>,
    phase: String,
    #[serde(default)]
    reason: Option<String>,
}

type Waiting = Arc<Mutex<HashMap<String, mpsc::Sender<Reply>>>>;

/// Nav's heading convention: 0 faces +Y, positive turns toward -X.
pub fn heading(from: [f64; 2], to: [f64; 2]) -> f64 {
    (-(to[0] - from[0])).atan2(to[1] - from[1])
}

/// Waypoints for nav: interim ones carry no heading, the last faces along the
/// final segment (from the robot when there is only one), as the runner does.
pub fn waypoints(robot: [f64; 2], points: &[[f64; 2]]) -> Result<(Vec<[f64; 2]>, f64)> {
    ensure!(!points.is_empty() && points.len() <= MAX_WAYPOINTS, "Send 1..{MAX_WAYPOINTS} waypoints");
    ensure!(points.iter().flatten().all(|v| v.is_finite() && v.abs() < 1e4), "Waypoints must be finite");
    let last = points[points.len() - 1];
    let from = if points.len() > 1 { points[points.len() - 2] } else { robot };
    ensure!((last[0] - from[0]).hypot(last[1] - from[1]) > 0.05, "Goal is where the robot already is");
    Ok((points.to_vec(), heading(from, last)))
}

struct Adapter {
    child: Child,
    stdin: Mutex<Option<ChildStdin>>,
    waiting: Waiting,
}
impl Adapter {
    fn ask(&self, request: serde_json::Value, request_id: &str) -> Result<Reply> {
        let (tx, rx) = mpsc::channel();
        self.waiting.lock().unwrap().insert(request_id.into(), tx);
        let result = (|| {
            let mut stdin = self.stdin.lock().unwrap();
            let pipe = stdin.as_mut().context("Nav adapter stopped")?;
            writeln!(pipe, "{request}").and_then(|_| pipe.flush()).context("Nav adapter unavailable")?;
            drop(stdin);
            let until = Instant::now() + REPLY_TIMEOUT;
            loop {
                let reply = rx
                    .recv_timeout(until.saturating_duration_since(Instant::now()))
                    .map_err(|_| anyhow!("Nav adapter did not answer in time"))?;
                if matches!(reply.phase.as_str(), "accepted" | "rejected" | "stopped") {
                    return Ok(reply);
                }
            }
        })();
        self.waiting.lock().unwrap().remove(request_id);
        result
    }
}
impl Drop for Adapter {
    fn drop(&mut self) {
        // Closing stdin ends the adapter; nav keeps its own state.
        self.stdin.lock().unwrap().take();
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Everything one navigation goal needs, shared by the Operables and the debug inbox.
struct Goals {
    known: Arc<Mutex<Frames>>,
    adapter: Arc<Adapter>,
    nav: FrameRef,
    base: FrameRef,
    clock: ClockReference,
    session: String,
}
impl Goals {
    fn go(&self, request: Navigate, caller: &str) -> Result<NavigateAccepted> {
        uuid::Uuid::parse_str(&request.request_id).context("request_id must be a UUID")?;
        let (points, h, resolved_from) = {
            let known = self.known.lock().map_err(|_| anyhow!("Frames unavailable"))?;
            let now = now_ns();
            let (nav_from_goal, mut used) = chain(&known, &request.frame, &self.nav, &self.clock, &self.session, now)?;
            let (nav_from_base, robot_used) = chain(&known, &self.base, &self.nav, &self.clock, &self.session, now)?;
            used.extend(robot_used);
            let robot = through(&nav_from_base, [0.; 3])?;
            let points = request
                .waypoints
                .iter()
                .map(|p| through(&nav_from_goal, *p).map(|q| [q[0], q[1]]))
                .collect::<Result<Vec<_>>>()?;
            let (points, h) = waypoints([robot[0], robot[1]], &points)?;
            (points, h, used)
        };
        let mut list: Vec<serde_json::Value> = points.iter().map(|p| serde_json::json!([p[0], p[1], null])).collect();
        let last = points[points.len() - 1];
        *list.last_mut().unwrap() = serde_json::json!([last[0], last[1], h]);
        let reply = self
            .adapter
            .ask(serde_json::json!({"op":"go","request_id":request.request_id,"waypoints":list}), &request.request_id)?;
        ensure!(reply.phase == "accepted", "{}", reply.reason.unwrap_or_else(|| "Nav refused the goal".into()));
        crate::diagnostic_log!(
            "navigate {} from {caller}: {} waypoints in {}, final heading {:.0}°",
            request.request_id, points.len(), request.frame.id, h.to_degrees()
        );
        Ok(NavigateAccepted {
            request_id: request.request_id,
            frame: request.frame,
            nav_frame: self.nav.clone(),
            waypoints_nav: points,
            final_heading_rad: h,
            executes: true,
            clock: self.clock.clone(),
            resolved_from,
        })
    }
    fn stop(&self, request_id: &str) -> Result<Cancelled> {
        uuid::Uuid::parse_str(request_id).context("request_id must be a UUID")?;
        let reply = self.adapter.ask(serde_json::json!({"op":"stop","request_id":request_id}), request_id)?;
        ensure!(reply.phase == "stopped", "{}", reply.reason.unwrap_or_else(|| "Nav did not stop".into()));
        Ok(Cancelled { request_id: request_id.into(), stopped: true })
    }
}

/// Local debug entry, reachable only by someone logged in on the robot: a
/// `navigate-debug/request.json` naming a frame this robot relates by its ID
/// and a point in it (or `"op": "stop"`) runs the same goal path as the
/// Operable; the outcome lands in `navigate-debug/response.json`.
pub const INBOX: &str = "navigate-debug";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DebugRequest {
    request_id: String,
    #[serde(default)]
    op: Option<String>,
    #[serde(default)]
    frame_id: Option<String>,
    #[serde(default)]
    point: Option<[f64; 3]>,
}
struct Inbox {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Inbox {
    fn start(goals: Arc<Goals>) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let dir = std::path::PathBuf::from(INBOX);
            let _ = std::fs::create_dir_all(&dir);
            while !stopping.load(std::sync::atomic::Ordering::Relaxed) {
                let request_path = dir.join("request.json");
                if let Ok(bytes) = std::fs::read(&request_path) {
                    let _ = std::fs::remove_file(&request_path);
                    let response = Self::handle(&goals, &bytes);
                    let temporary = dir.join("response.tmp");
                    if std::fs::write(&temporary, serde_json::to_vec_pretty(&response).unwrap_or_default()).is_ok() {
                        let _ = std::fs::rename(&temporary, dir.join("response.json"));
                    }
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        });
        Self { stop, thread: Some(thread) }
    }
    fn handle(goals: &Goals, bytes: &[u8]) -> serde_json::Value {
        let run = || -> Result<serde_json::Value> {
            ensure!(bytes.len() <= 4096, "Debug request too large");
            let request: DebugRequest = serde_json::from_slice(bytes)?;
            if request.op.as_deref() == Some("stop") {
                return Ok(serde_json::to_value(goals.stop(&request.request_id)?)?);
            }
            ensure!(request.op.as_deref().is_none_or(|op| op == "go"), "op must be go or stop");
            let id = request.frame_id.context("frame_id is required")?;
            let point = request.point.context("point is required")?;
            let frame = {
                let known = goals.known.lock().map_err(|_| anyhow!("Frames unavailable"))?;
                let found = known.find(&id);
                match found.as_slice() {
                    [one] => one.clone(),
                    [] => anyhow::bail!("This robot relates no frame {id}; is it aligned to that scene?"),
                    _ => anyhow::bail!("Several frames are called {id}"),
                }
            };
            let accepted = goals.go(Navigate { request_id: request.request_id, frame, waypoints: vec![point] }, "local debug")?;
            Ok(serde_json::to_value(accepted)?)
        };
        match run() {
            Ok(result) => serde_json::json!({"ok": true, "result": result}),
            Err(e) => serde_json::json!({"ok": false, "error": format!("{e:#}")}),
        }
    }
}
impl Drop for Inbox {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub struct Navigator {
    _inbox: Inbox,
    _component: Component,
    _navigate: Operable<Navigate, NavigateAccepted>,
    _cancel: Operable<Cancel, Cancelled>,
    _mount: Component,
    _capture: BufferProductCapture<Update>,
}

impl Navigator {
    pub fn mount(p: &crate::poses::Publisher, known: Arc<Mutex<Frames>>, home: String) -> Result<Self> {
        let mut child = Command::new("python3")
            .args(["scripts/bracketbot-python.py", "scripts/diagnostic-nav.py"])
            .env_remove("AUKI_ACCESS_KEY")
            .env_remove("AUKI_APP_SECRET")
            .env_remove("AUKI_EMAIL")
            .env_remove("AUKI_PASSWORD")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let (first_tx, first_rx) = mpsc::channel();
        let waiting: Waiting = Arc::default();
        let replies = waiting.clone();
        std::thread::spawn(move || {
            let _ = first_tx.send(lines.next());
            for line in lines {
                let Ok(line) = line else { break };
                let Ok(reply) = serde_json::from_str::<Reply>(&line) else {
                    continue; // BBOS libraries print status lines on stdout too.
                };
                crate::diagnostic_log!("Nav adapter: {line}");
                if let Some(id) = &reply.request_id
                    && let Some(tx) = replies.lock().unwrap().get(id)
                {
                    let _ = tx.send(reply.clone());
                }
            }
            replies.lock().unwrap().clear();
        });
        let declaration = (|| -> Result<Declaration> {
            let first = first_rx
                .recv_timeout(Duration::from_secs(30))
                .map_err(|_| anyhow!("Nav adapter did not declare its frame"))?
                .context("Nav adapter ended")??;
            let declaration: Declaration = serde_json::from_str(&first)?;
            declaration.nav_from_world.validate()?;
            ensure!(matches!(declaration.mode.as_str(), "daemon" | "app-map" | "app-mapless"), "Unknown nav mode");
            Ok(declaration)
        })();
        let declaration = match declaration {
            Ok(d) => d,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        crate::diagnostic_log!("Nav backend: {} (its frame relative to the SLAM world as BBOS defines it)", declaration.mode);
        let adapter = Arc::new(Adapter { child, stdin: Mutex::new(stdin), waiting });
        // Nav's floor frame: Z up, metres, its +Y the robot's forward at heading 0.
        let owner = &p.contract.from_frame.peer_id;
        let nav_frame = FrameRegistryEntry::in_convention(
            owner,
            format!("bbos-nav-{}", p.contract.session_id),
            CoordinateConvention::ZUpRightForward,
        );
        p.definitions.lock().map_err(|_| anyhow!("Registry unavailable"))?.push((
            RegistryKind::Frame,
            nav_frame.frame_id.clone(),
            nav_frame.hash(),
            String::from_utf8(nav_frame.canonical_bytes())?,
        ));
        let held = Contract {
            from_frame: (&p.contract.to_frame).into(),
            to_frame: (&nav_frame).into(),
            clock: p.contract.clock.clone(),
            session_id: p.contract.session_id.clone(),
            lifetime: Lifetime::Held,
        };
        let mount = p.runtime.component(ComponentSpec::new(MOUNT).observable(ObservableContract {
            name: "transform".into(),
            datatype: crate::compact_pose::SCHEMA.into(),
            schema: crate::compact_pose::SCHEMA.into(),
            access: vec![ObservationAccess::FollowNew],
            exposure: Exposure::Cluster,
        }))?;
        let output = mount.configured_observable(
            ConfiguredObservableSpec::new(
                "transform",
                held.output_id(),
                held.clock.clone(),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "pose".into(),
                    datatype: crate::compact_pose::SCHEMA.into(),
                    schema: crate::compact_pose::SCHEMA.into(),
                    observes: "SLAM world in the nav app's frame".into(),
                    unit: None,
                }),
            )
            .in_registered_frame(nav_frame.clone()),
        )?;
        held.validate_output(&output.manifest())?;
        mount.expose()?;
        let bound = held.clone();
        let capture = p.runtime.capture_buffer_with_metadata(
            MOUNT,
            &output,
            BufferLimits::entries(1),
            |_: &Update| 256,
            move |o| {
                Ok(Some(CatalogProductMetadata {
                    schema: crate::compact_pose::METADATA.into(),
                    source_sequence: o.sequence,
                    value: serde_json::to_value(&bound).map_err(|e| e.to_string())?,
                }))
            },
        )?;
        p.endpoint.export_product(&capture.product())?;
        output.publish(
            0,
            Arc::new(Update {
                tracking: auki_odometry::Tracking::Valid { pose: declaration.nav_from_world },
            }),
        )?;
        known
            .lock()
            .unwrap()
            .insert(&[&p.contract.to_frame, &nav_frame], Transform { contract: held, product: capture.product() });

        let component = p.runtime.component(
            ComponentSpec::new(COMPONENT).operable(contract()).operable(cancel_contract()),
        )?;
        let goals = Arc::new(Goals {
            known,
            adapter,
            nav: (&nav_frame).into(),
            base: (&p.contract.from_frame).into(),
            clock: p.contract.clock.clone(),
            session: p.contract.session_id.clone(),
        });
        let (only_home, also_home) = (home.clone(), home);
        let (go, stop) = (goals.clone(), goals.clone());
        let rejected = |e: anyhow::Error| InvocationError::Rejected(format!("{e:#}"));
        let navigate = component.operable_ordered(
            OPERATION,
            InvocationOrdering::SerialInAcceptanceOrder,
            // Navigation goals come only from the Home this robot is paired with.
            move |ctx| ctx.caller_peer_id == only_home,
            move |ctx, request: Navigate| go.go(request, &ctx.caller_peer_id).map_err(rejected),
        )?;
        let cancel = component.operable_ordered(
            CANCEL,
            InvocationOrdering::SerialInAcceptanceOrder,
            move |ctx| ctx.caller_peer_id == also_home,
            move |_ctx, request: Cancel| stop.stop(&request.request_id).map_err(rejected),
        )?;
        let inbox = Inbox::start(goals);
        component.expose()?;
        p.endpoint.export_operable(&navigate)?;
        p.endpoint.export_operable(&cancel)?;
        Ok(Self { _component: component, _navigate: navigate, _cancel: cancel, _mount: mount, _capture: capture, _inbox: inbox })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headings_follow_navs_convention() {
        let close = |a: f64, b: f64| assert!((a - b).abs() < 1e-9, "{a} vs {b}");
        close(heading([0., 0.], [0., 1.]), 0.); // +Y
        close(heading([0., 0.], [-1., 0.]), std::f64::consts::FRAC_PI_2); // -X is +90°
        close(heading([0., 0.], [1., 0.]), -std::f64::consts::FRAC_PI_2);
        close(heading([0., 0.], [0., -1.]).abs(), std::f64::consts::PI);
    }
    #[test]
    fn the_last_waypoint_faces_along_the_final_segment() {
        let (points, h) = waypoints([0., 0.], &[[0., 2.]]).unwrap();
        assert_eq!(points, vec![[0., 2.]]);
        assert!(h.abs() < 1e-9, "from the robot along +Y");
        let (_, h) = waypoints([5., 5.], &[[0., 2.], [-1., 2.]]).unwrap();
        assert!((h - std::f64::consts::FRAC_PI_2).abs() < 1e-9, "from the previous waypoint");
        assert!(waypoints([0., 0.], &[]).is_err());
        assert!(waypoints([0., 0.], &[[0.01, 0.]]).is_err(), "already there");
        assert!(waypoints([0., 0.], &[[f64::NAN, 1.]]).is_err());
    }
}
