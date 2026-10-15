//! `navigate` Operable contract, shared by every robot that offers it and every
//! caller. Waypoints name their frame by full Registry reference; the robot
//! resolves them only through transforms it publishes (as `point_at` does) into
//! the frame of the navigation stack that drives it, and hands that stack the
//! list. The robot's own stack plans locally, avoids obstacles and drives.
use crate::compact_pose::FrameRef;
use crate::point_at::Used;
use auki_components::{ClockReference, ContractType, Exposure, OperableContract};
use serde::{Deserialize, Serialize};

pub const COMPONENT: &str = "base-navigator";
pub const OPERATION: &str = "navigate";
pub const CANCEL: &str = "cancel";
pub const INSTRUCTION: &str = "spatial-harness.navigate/v1";
pub const RESULT: &str = "spatial-harness.navigate-accepted/v1";
pub const CANCEL_INSTRUCTION: &str = "spatial-harness.navigate-cancel/v1";
pub const CANCEL_RESULT: &str = "spatial-harness.navigate-cancelled/v1";
pub const MAX_WAYPOINTS: usize = 64;

pub fn contract() -> OperableContract {
    OperableContract {
        name: OPERATION.into(),
        instruction: INSTRUCTION.into(),
        result: RESULT.into(),
        exposure: Exposure::Cluster,
    }
}
pub fn cancel_contract() -> OperableContract {
    OperableContract {
        name: CANCEL.into(),
        instruction: CANCEL_INSTRUCTION.into(),
        result: CANCEL_RESULT.into(),
        exposure: Exposure::Cluster,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Navigate {
    pub request_id: String,
    /// A frame the robot relates to its navigation frame through its published transforms.
    pub frame: FrameRef,
    /// Floor points in `frame`, in order, ending at the goal. Heights are ignored.
    pub waypoints: Vec<[f64; 3]>,
}
impl ContractType for Navigate {
    const DATATYPE: &'static str = INSTRUCTION;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigateAccepted {
    pub request_id: String,
    pub frame: FrameRef,
    /// The frame the robot's navigation stack drives in.
    pub nav_frame: FrameRef,
    /// The waypoints in `nav_frame`, on its floor, in metres.
    pub waypoints_nav: Vec<[f64; 2]>,
    /// Final heading in `nav_frame`, radians about its up axis.
    pub final_heading_rad: f64,
    /// False when the robot only resolves goals and drives nothing.
    pub executes: bool,
    /// The clock of every timestamp in `resolved_from`.
    pub clock: ClockReference,
    pub resolved_from: Vec<Used>,
}
impl ContractType for NavigateAccepted {
    const DATATYPE: &'static str = RESULT;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cancel {
    pub request_id: String,
}
impl ContractType for Cancel {
    const DATATYPE: &'static str = CANCEL_INSTRUCTION;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cancelled {
    pub request_id: String,
    /// True when the robot told its navigation stack to stop.
    pub stopped: bool,
}
impl ContractType for Cancelled {
    const DATATYPE: &'static str = CANCEL_RESULT;
}
