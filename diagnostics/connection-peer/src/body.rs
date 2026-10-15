//! Bracketbot body state: `bbos-joints`, `bbos-health`, `bbos-battery` and
//! `bbos-pointing`, each declaring the body model rooted at the published pose's
//! base frame, on the pose clock. Every observation is stamped with the capture
//! time of what it holds: one arm's state, the battery, or a pointing phase.
use anyhow::{Result, ensure};
use auki_components::*;
use serde::Deserialize;
use std::sync::Arc;
use crate::robot_state::{Battery, BodyContract, Health, JointHealth, Joints, Pointing, BATTERY_SCHEMA, HEALTH_SCHEMA, JOINTS_SCHEMA, METADATA, POINTING_SCHEMA};
use std::collections::BTreeMap;

/// The URDF the Home viewer serves for this body.
const MODEL: &str = "bracketbot";
const JOINTS: &str = "bbos-joints";
const HEALTH: &str = "bbos-health";
const BATTERY: &str = "bbos-battery";
const POINTING: &str = "bbos-pointing";

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Sample {
    /// One arm's positions, at its BBOS arm-state capture time.
    Joints { timestamp_ns: u64, positions: BTreeMap<String, f64> },
    /// One arm's motor readings, at its BBOS arm-state capture time.
    Health { timestamp_ns: u64, joints: BTreeMap<String, JointHealth> },
    /// The battery, at its BBOS `drive.status` capture time.
    Battery { timestamp_ns: u64, battery: Battery },
}

pub fn start() -> (tokio::task::JoinHandle<()>, tokio::sync::mpsc::Receiver<Sample>) {
    // Joints and health interleave on one pipe; a watch would drop one kind.
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let task = tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let work = async {
            let mut child = tokio::process::Command::new("python3")
                .args(["scripts/bracketbot-python.py", "scripts/diagnostic-robot-state.py"])
                .env_remove("AUKI_ACCESS_KEY").env_remove("AUKI_APP_SECRET")
                .env_remove("AUKI_EMAIL").env_remove("AUKI_PASSWORD")
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped())
                .kill_on_drop(true).spawn()?;
            let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
            loop {
                let line = lines.next_line().await?.ok_or_else(|| anyhow::anyhow!("Robot state capture ended"))?;
                ensure!(line.len() <= 16 * 1024, "Robot state IPC sample too large");
                // Latest-only: drop a sample rather than stall the helper.
                let _ = tx.try_send(serde_json::from_str::<Sample>(&line)?);
            }
            #[allow(unreachable_code)] Ok::<(), anyhow::Error>(())
        };
        if let Err(e) = work.await { eprintln!("Robot state capture failed: {e}"); }
    });
    (task, rx)
}

struct Stream<T: ContractType + Send + Sync + 'static> {
    output: ConfiguredObservable<T>,
    capture: BufferProductCapture<T>,
    _component: Component,
    last: u64,
}

fn stream<T>(p: &crate::poses::Publisher, body: &BodyContract, product: &str, schema: &str, observes: &str)
    -> Result<Stream<T>> where T: ContractType + serde::Serialize + Send + Sync + 'static {
    let kind = product.trim_start_matches("bbos-");
    let component = p.runtime.component(ComponentSpec::new(product).observable(ObservableContract {
        name: kind.into(), datatype: schema.into(), schema: schema.into(),
        access: vec![ObservationAccess::FollowNew], exposure: Exposure::Cluster }))?;
    let output = component.configured_observable(ConfiguredObservableSpec::new(kind, body.output_id(kind), body.clock.clone(),
        PayloadContract::Structured(StructuredPayloadContract {datatype: schema.into(), schema: schema.into(),
            modality: "robot_state".into(), observes: observes.into(), unit: None})))?;
    component.expose()?;
    let declared = serde_json::to_value(body)?;
    let capture = p.runtime.capture_buffer_with_metadata(product, &output,
        BufferLimits {max_entries: Some(4), max_bytes: Some(64 * 1024), target_duration: None},
        |v: &T| serde_json::to_vec(v).map_or(usize::MAX, |b| b.len()),
        move |o| Ok(Some(CatalogProductMetadata {schema: METADATA.into(), source_sequence: o.sequence, value: declared.clone()})))?;
    p.endpoint.export_product(&capture.product())?;
    Ok(Stream {output, capture, _component: component, last: 0})
}

impl<T: ContractType + Send + Sync + 'static> Stream<T> {
    fn publish(&mut self, timestamp_ns: u64, value: T) -> Result<()> {
        // Never replay or reorder a timeline.
        if timestamp_ns <= self.last { return Ok(()); }
        self.output.publish(timestamp_ns, Arc::new(value))?;
        ensure!(self.capture.errors().is_empty(), "Robot state retention failed");
        self.last = timestamp_ns;
        Ok(())
    }
}

pub struct Body {
    joints: Stream<Joints>,
    health: Stream<Health>,
    battery: Stream<Battery>,
    pointing: Stream<Pointing>,
}
impl Body {
    pub fn mount(p: &crate::poses::Publisher) -> Result<Self> {
        let body = BodyContract {model: MODEL.into(), base_frame: (&p.contract.from_frame).into(),
            clock: p.contract.clock.clone(), session_id: p.contract.session_id.clone()};
        body.validate()?;
        Ok(Self {
            joints: stream(p, &body, JOINTS, JOINTS_SCHEMA, "urdf_joint_positions")?,
            health: stream(p, &body, HEALTH, HEALTH_SCHEMA, "motor_health")?,
            battery: stream(p, &body, BATTERY, BATTERY_SCHEMA, "battery")?,
            pointing: stream(p, &body, POINTING, POINTING_SCHEMA, "pointing_request_status")?,
        })
    }
    pub fn publish(&mut self, sample: Sample) -> Result<()> {
        match sample {
            Sample::Joints {timestamp_ns, positions} => {
                let joints = Joints {positions};
                joints.validate()?;
                self.joints.publish(timestamp_ns, joints)
            }
            Sample::Health {timestamp_ns, joints} => {
                let health = Health {joints, battery: None};
                health.validate()?;
                self.health.publish(timestamp_ns, health)
            }
            Sample::Battery {timestamp_ns, battery} => {
                battery.validate()?;
                self.battery.publish(timestamp_ns, battery)
            }
        }
    }
    /// A pointing phase at the time it happened, on the capture clock (UTC ns
    /// on this robot). A phase no later than the last one is refused, not restamped.
    pub fn publish_pointing(&mut self, timestamp_ns: u64, pointing: Pointing) -> Result<()> {
        pointing.validate()?;
        ensure!(timestamp_ns > self.pointing.last, "Pointing phase {} is not after the previous one", pointing.phase);
        self.pointing.publish(timestamp_ns, pointing)
    }
    pub fn close(self, p: &crate::poses::Publisher) -> Result<()> {
        p.endpoint.unexport_product(JOINTS);
        p.endpoint.unexport_product(HEALTH);
        p.endpoint.unexport_product(BATTERY);
        p.endpoint.unexport_product(POINTING);
        self.joints.capture.delete()?;
        self.health.capture.delete()?;
        self.battery.capture.delete()?;
        self.pointing.capture.delete()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_lines_parse_into_typed_state() {
        let joints: Sample = serde_json::from_str(r#"{"kind":"joints","timestamp_ns":5,"positions":{"left_shoulder":0.5}}"#).unwrap();
        assert!(matches!(joints, Sample::Joints {timestamp_ns: 5, ..}));
        let health: Sample = serde_json::from_str(r#"{"kind":"health","timestamp_ns":6,"joints":{"left_shoulder":{"temperature_c":41.0,"current_a":0.2}}}"#).unwrap();
        let Sample::Health {joints, ..} = health else { panic!() };
        Health {joints, battery: None}.validate().unwrap();
        // The battery has its own capture time, so it never rides in a health sample.
        assert!(serde_json::from_str::<Sample>(r#"{"kind":"health","timestamp_ns":6,"joints":{},"battery":null}"#).is_err());
        let battery: Sample = serde_json::from_str(r#"{"kind":"battery","timestamp_ns":7,"battery":{"voltage":18.5,"percent":90.0}}"#).unwrap();
        assert!(matches!(battery, Sample::Battery {timestamp_ns: 7, ..}));
    }
}
