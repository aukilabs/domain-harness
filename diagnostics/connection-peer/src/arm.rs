//! The local arm adapter behind `point_at`: one Python process over
//! stdin/stdout IPC, the same pattern as the camera and depth adapters. It
//! declares where the arm IK frame sits in the base, and this peer publishes
//! that as a held transform so targets reach the arm only through it.
use crate::compact_pose::{Contract, Lifetime, Update};
use crate::point_at::ArmChoice;
use anyhow::{Context, Result, anyhow};
use auki_components::*;
use auki_protocols::registry::v3::RegistryKind;
use auki_registry::FrameRegistryEntry;
use serde::Deserialize;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

pub const MOUNT: &str = "bbos-arm-mount";
/// A move is ready, point, hold and retrace (a few seconds each way); it finishes well inside this.
pub const MOTION_TIMEOUT: Duration = Duration::from_secs(60);
const PLAN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    arm_base_to_base: auki_odometry::Pose,
}

/// One adapter line about a request; other fields are reports for the log.
#[derive(Clone, Debug, Deserialize)]
pub struct Reply {
    pub request_id: Option<String>,
    pub phase: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub executes: Option<bool>,
    #[serde(default)]
    pub arm: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    /// When this phase happened, on this host's realtime clock (as BBOS stamps
    /// captures); the body publisher converts it into the session clock.
    #[serde(default)]
    pub timestamp_ns: Option<u64>,
}
impl Reply {
    /// The request no longer holds the arm.
    pub fn finishes(&self) -> bool {
        matches!(self.phase.as_str(), "done" | "fault" | "rejected")
            || self.phase == "planned" && self.executes != Some(true)
    }
}

type Waiting = Arc<Mutex<HashMap<String, mpsc::Sender<Reply>>>>;

pub struct Arm {
    pub frame: FrameRegistryEntry,
    pub transform: crate::pointer::Transform,
    pub executes: bool,
    child: Child,
    stdin: Mutex<Option<ChildStdin>>,
    waiting: Waiting,
    gone: Arc<std::sync::atomic::AtomicBool>,
    _component: Component,
    _capture: BufferProductCapture<Update>,
}

impl Arm {
    /// Start the adapter and publish its declared arm frame. `finished` is
    /// called with a request ID when that request releases the arm, and with
    /// `None` when the adapter is gone.
    pub fn start(
        p: &crate::poses::Publisher,
        execute: bool,
        finished: impl Fn(Option<&str>) + Send + 'static,
        reports: tokio::sync::mpsc::UnboundedSender<Reply>,
    ) -> Result<Self> {
        let mut command = Command::new("python3");
        command.args(["scripts/bracketbot-python.py", "scripts/diagnostic-arm.py"]);
        if execute {
            command.arg("--execute");
        }
        let mut child = command
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
        let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ended = gone.clone();
        std::thread::spawn(move || {
            let _ = first_tx.send(lines.next());
            for line in lines {
                let Ok(line) = line else { break };
                let Ok(reply) = serde_json::from_str::<Reply>(&line) else {
                    crate::diagnostic_log!("Arm adapter: unreadable line");
                    continue;
                };
                crate::diagnostic_log!("Arm adapter: {line}");
                if reply.request_id.is_some() {
                    let _ = reports.send(reply.clone());
                }
                if let Some(id) = &reply.request_id {
                    if let Some(tx) = replies.lock().unwrap().get(id) {
                        let _ = tx.send(reply.clone());
                    }
                    if reply.finishes() {
                        finished(Some(id));
                    }
                }
            }
            replies.lock().unwrap().clear();
            ended.store(true, std::sync::atomic::Ordering::Relaxed);
            finished(None);
        });
        let started = (|| -> Result<_> {
            let first = first_rx
                .recv_timeout(Duration::from_secs(30))
                .map_err(|_| anyhow!("Arm adapter did not declare its frame"))?
                .context("Arm adapter ended")??;
            let declaration: Declaration = serde_json::from_str(&first)?;
            declaration.arm_base_to_base.validate()?;
            Ok(declaration)
        })();
        let declaration = match started {
            Ok(d) => d,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        let owner = &p.contract.from_frame.peer_id;
        let frame = FrameRegistryEntry::ros_body(owner, format!("bbos-arm-base-{}", p.contract.session_id));
        p.definitions.lock().map_err(|_| anyhow!("Registry unavailable"))?.push((
            RegistryKind::Frame,
            frame.frame_id.clone(),
            frame.hash(),
            String::from_utf8(frame.canonical_bytes())?,
        ));
        let contract = Contract {
            from_frame: (&frame).into(),
            to_frame: (&p.contract.from_frame).into(),
            clock: p.contract.clock.clone(),
            session_id: p.contract.session_id.clone(),
            lifetime: Lifetime::Held,
        };
        let component = p.runtime.component(ComponentSpec::new(MOUNT).observable(ObservableContract {
            name: "transform".into(),
            datatype: crate::compact_pose::SCHEMA.into(),
            schema: crate::compact_pose::SCHEMA.into(),
            access: vec![ObservationAccess::FollowNew],
            exposure: Exposure::Cluster,
        }))?;
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "transform",
                contract.output_id(),
                contract.clock.clone(),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "pose".into(),
                    datatype: crate::compact_pose::SCHEMA.into(),
                    schema: crate::compact_pose::SCHEMA.into(),
                    observes: "arm IK frame to base".into(),
                    unit: None,
                }),
            )
            .in_registered_frame(p.contract.from_frame.clone()),
        )?;
        contract.validate_output(&output.manifest())?;
        component.expose()?;
        let bound = contract.clone();
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
        // A fixed mount: valid for this capture session, like the head extrinsic.
        output.publish(
            0,
            Arc::new(Update {
                tracking: auki_odometry::Tracking::Valid {
                    pose: declaration.arm_base_to_base,
                },
            }),
        )?;
        Ok(Self {
            frame,
            transform: crate::pointer::Transform {
                contract,
                product: capture.product(),
            },
            executes: execute,
            child,
            stdin: Mutex::new(stdin),
            waiting,
            gone,
            _component: component,
            _capture: capture,
        })
    }

    /// The adapter process has exited.
    pub fn gone(&self) -> bool {
        self.gone.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Ask the adapter to plan (and, if it executes, perform) one request.
    /// Returns its "planned" or "rejected" reply.
    pub fn plan(&self, request_id: &str, arm: ArmChoice, target_arm_base: [f64; 3]) -> Result<Reply> {
        let side = match arm {
            ArmChoice::Left => "left",
            ArmChoice::Right => "right",
            ArmChoice::Auto => anyhow::bail!("Arm must be chosen before planning"),
        };
        let (tx, rx) = mpsc::channel();
        self.waiting.lock().unwrap().insert(request_id.into(), tx);
        let result = (|| {
            let line = serde_json::to_string(&serde_json::json!({
                "request_id": request_id, "arm": side, "target_arm_base": target_arm_base,
            }))?;
            let mut stdin = self.stdin.lock().unwrap();
            let pipe = stdin.as_mut().context("Arm adapter stopped")?;
            writeln!(pipe, "{line}").and_then(|_| pipe.flush()).context("Arm adapter unavailable")?;
            drop(stdin);
            let until = Instant::now() + PLAN_TIMEOUT;
            loop {
                let left = until.saturating_duration_since(Instant::now());
                let reply = rx.recv_timeout(left).map_err(|_| anyhow!("Arm adapter did not plan in time"))?;
                if matches!(reply.phase.as_str(), "planned" | "rejected") {
                    return Ok(reply);
                }
            }
        })();
        self.waiting.lock().unwrap().remove(request_id);
        result
    }
}

impl Drop for Arm {
    fn drop(&mut self) {
        // Closing stdin ends the adapter after any move in progress has
        // returned the arm to where it started; never kill it mid-move unless it hangs.
        self.stdin.lock().unwrap().take();
        let until = Instant::now() + MOTION_TIMEOUT;
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

#[cfg(test)]
mod tests {
    use super::*;
    fn reply(phase: &str, executes: Option<bool>) -> Reply {
        Reply { request_id: Some("r".into()), phase: phase.into(), reason: None, executes, arm: None, code: None, timestamp_ns: Some(1) }
    }
    #[test]
    fn a_request_holds_the_arm_until_its_move_ends() {
        assert!(!reply("planned", Some(true)).finishes());
        assert!(!reply("moving", None).finishes());
        assert!(!reply("holding", None).finishes());
        assert!(reply("planned", Some(false)).finishes());
        for end in ["done", "fault", "rejected"] {
            assert!(reply(end, None).finishes());
        }
        let parsed: Reply =
            serde_json::from_str(r#"{"request_id":"r","phase":"holding","measured_hand_m":[1,2,3]}"#).unwrap();
        assert_eq!(parsed.phase, "holding");
    }
}
