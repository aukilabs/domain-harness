//! Robot body state over SDK Products: which body model sits on a pose frame,
//! its joint positions, and its motor and battery health. Shared by Home and
//! every publisher; nothing here is specific to one robot.
//!
//! The publisher declares the body in Product metadata. A reader draws that
//! model wherever the declared base frame's pose places it, so a body is found
//! by its frame reference, never by a peer, product or device name.
use crate::compact_pose::FrameRef;
use anyhow::{Result, ensure};
use auki_components::{ClockReference, ContractType, manifest_hash};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const JOINTS_SCHEMA: &str = "spatial-harness.robot-joints/v1";
pub const HEALTH_SCHEMA: &str = "spatial-harness.robot-health/v1";
pub const POINTING_SCHEMA: &str = "spatial-harness.robot-pointing/v1";
pub const METADATA: &str = "spatial-harness.robot-body/v1";
const MAX_JOINTS: usize = 64;

/// The body `model` whose root link is `base_frame`, on the publisher's clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyContract {
    pub model: String,
    pub base_frame: FrameRef,
    pub clock: ClockReference,
    pub session_id: String,
}
impl BodyContract {
    /// Bind the descriptor into the output ID, as the compact pose profile does.
    pub fn output_id(&self, kind: &str) -> String {
        format!("{kind}-{}", manifest_hash(self))
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.model.is_empty() && self.model.len() <= 64
            && self.model.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "Invalid body model name");
        ensure!(!self.session_id.trim().is_empty() && self.session_id.len() <= 256, "Invalid body session");
        ensure!(self.base_frame.peer_id == self.clock.peer_id, "Body frame and clock owner differ");
        Ok(())
    }
}

fn joint_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64
}

/// Joint positions keyed by URDF joint name: radians, or metres for prismatic joints.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Joints {
    pub positions: BTreeMap<String, f64>,
}
impl ContractType for Joints {
    const DATATYPE: &'static str = JOINTS_SCHEMA;
}
impl Joints {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.positions.is_empty() && self.positions.len() <= MAX_JOINTS
            && self.positions.iter().all(|(name, v)| joint_name(name) && v.is_finite()),
            "Invalid robot joints");
        Ok(())
    }
}

/// Motor health keyed like `Joints`, and the battery. Presentation only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    #[serde(default)]
    pub joints: BTreeMap<String, JointHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<Battery>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointHealth {
    pub temperature_c: f64,
    pub current_a: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Battery {
    pub voltage: f64,
    pub percent: f64,
}
impl ContractType for Health {
    const DATATYPE: &'static str = HEALTH_SCHEMA;
}
impl Health {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.joints.is_empty() || self.battery.is_some(), "Empty robot health");
        ensure!(self.joints.len() <= MAX_JOINTS && self.joints.iter().all(|(name, j)|
            joint_name(name) && j.temperature_c.is_finite() && j.current_a.is_finite()),
            "Invalid joint health");
        if let Some(b) = &self.battery {
            ensure!(b.voltage.is_finite() && b.voltage >= 0. && (0. ..=100.).contains(&b.percent), "Invalid battery");
        }
        Ok(())
    }
}

/// The latest pointing request: its phase, and why it was refused or stopped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pointing {
    pub request_id: String,
    /// planned, moving, holding, returning, done, rejected or fault.
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// A short cause for alerts: hot, current, unreachable or other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}
impl ContractType for Pointing {
    const DATATYPE: &'static str = POINTING_SCHEMA;
}
impl Pointing {
    pub fn validate(&self) -> Result<()> {
        const PHASES: [&str; 7] = ["planned", "moving", "holding", "returning", "done", "rejected", "fault"];
        ensure!(!self.request_id.is_empty() && self.request_id.len() <= 128, "Invalid pointing request ID");
        ensure!(PHASES.contains(&self.phase.as_str()), "Unknown pointing phase");
        ensure!(self.arm.as_ref().is_none_or(|a| joint_name(a)), "Invalid pointing arm");
        ensure!(self.reason.as_ref().is_none_or(|r| r.len() <= 300), "Pointing reason too long");
        ensure!(self.code.as_ref().is_none_or(|c| !c.is_empty() && c.len() <= 32
            && c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')), "Invalid pointing code");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn body() -> BodyContract {
        BodyContract {model:"bracketbot".into(),
            base_frame:FrameRef{peer_id:"p".into(),id:"bbos-base".into(),hash:"a".into()},
            clock:ClockReference{peer_id:"p".into(),id:"bbos-capture".into(),hash:"b".into()},
            session_id:"s".into()}
    }
    #[test]
    fn output_identity_binds_the_declared_body() {
        let mut other = body();
        other.model = "another".into();
        assert_ne!(body().output_id("joints"), other.output_id("joints"));
        assert_ne!(body().output_id("joints"), body().output_id("health"));
    }
    #[test]
    fn rejects_foreign_frames_and_unsafe_model_names() {
        body().validate().unwrap();
        let mut foreign = body();
        foreign.base_frame.peer_id = "other".into();
        assert!(foreign.validate().is_err());
        let mut path = body();
        path.model = "../robot".into();
        assert!(path.validate().is_err());
    }
    #[test]
    fn rejects_empty_or_invalid_state() {
        assert!(Joints{positions:BTreeMap::new()}.validate().is_err());
        assert!(Joints{positions:[("j".into(),f64::NAN)].into()}.validate().is_err());
        assert!(Health{joints:BTreeMap::new(),battery:None}.validate().is_err());
        assert!(Health{joints:BTreeMap::new(),battery:Some(Battery{voltage:19.,percent:101.})}.validate().is_err());
        Health{joints:[("j".into(),JointHealth{temperature_c:40.,current_a:-0.5})].into(),battery:None}.validate().unwrap();
        assert!(serde_json::from_str::<Health>(r#"{"timestamp_ns":1,"joints":{}}"#).is_err());
    }
    #[test]
    fn pointing_status_names_known_phases_and_short_codes() {
        let fault = Pointing {request_id:"r".into(), phase:"fault".into(), arm:Some("arm_left".into()),
            reason:Some("lj0 at 50 C (limit 50 C)".into()), code:Some("hot".into())};
        fault.validate().unwrap();
        assert!(Pointing {phase:"exploded".into(), ..fault.clone()}.validate().is_err());
        assert!(Pointing {code:Some("Hot!".into()), ..fault}.validate().is_err());
    }
}
