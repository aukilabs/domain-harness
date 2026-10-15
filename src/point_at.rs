//! `point_at` Operable contract, shared by every robot that offers it and every
//! caller. A target names its frame by full Registry reference; the robot
//! resolves it only through transforms it publishes under the compact pose
//! profile, so a caller can see beforehand which frames a robot can point in.
use crate::compact_pose::FrameRef;
use auki_components::{ClockReference, ContractType, Exposure, OperableContract, OutputReference};
use serde::{Deserialize, Serialize};

pub const COMPONENT: &str = "arm-pointer";
pub const OPERATION: &str = "point_at";
pub const INSTRUCTION: &str = "spatial-harness.point-at/v1";
pub const RESULT: &str = "spatial-harness.point-at-accepted/v1";

pub fn contract() -> OperableContract {
    OperableContract {
        name: OPERATION.into(),
        instruction: INSTRUCTION.into(),
        result: RESULT.into(),
        exposure: Exposure::Cluster,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmChoice {
    /// The arm on the target's side of the robot.
    #[default]
    Auto,
    Left,
    Right,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointAt {
    pub request_id: String,
    /// A frame the robot relates to its base through its published transforms.
    pub frame: FrameRef,
    /// The target in `frame`'s coordinates and declared units.
    pub point: [f64; 3],
    #[serde(default)]
    pub arm: ArmChoice,
    /// Drive these waypoints (in `frame`) first, end facing the target, then
    /// point. Empty points from where the robot stands. Robots take a route
    /// only from the Home they are paired with, as for `navigate`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approach: Vec<[f64; 3]>,
}
impl ContractType for PointAt {
    const DATATYPE: &'static str = INSTRUCTION;
}

/// One transform observation the resolution used.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Used {
    pub output: OutputReference,
    pub sequence: u64,
    pub timestamp_ns: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointAtAccepted {
    pub request_id: String,
    pub arm: ArmChoice,
    pub frame: FrameRef,
    pub base_frame: FrameRef,
    /// The target in `base_frame` at acceptance, in metres.
    pub point_in_base: [f64; 3],
    pub distance_m: f64,
    /// Horizontal angle from the base's forward axis; positive is to its left.
    pub bearing_deg: f64,
    /// Angle above the base's horizontal plane.
    pub elevation_deg: f64,
    pub hold_s: f64,
    /// False until arm motion is attached; the request is resolved only.
    pub executes: bool,
    /// The clock of every timestamp in `resolved_from`.
    pub clock: ClockReference,
    pub resolved_from: Vec<Used>,
}
impl ContractType for PointAtAccepted {
    const DATATYPE: &'static str = RESULT;
}
