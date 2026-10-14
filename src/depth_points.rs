//! Shared declared point-cloud wire contract for every producer.
use crate::compact_pose::FrameRef;
use anyhow::{Result, ensure};
use auki_components::*;
use serde::{Deserialize, Serialize};

pub const POINTS_SCHEMA: &str = "spatial-harness.depth-points/v1";
pub const POINTS_METADATA: &str = "spatial-harness.depth-points-contract/v1";
pub const MAX_POINTS: usize = 2048;

/// Which frame the points are in. A reader places them by the transforms out of that frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointsContract {
    pub frame: FrameRef,
    pub clock: ClockReference,
    pub session_id: String,
}
impl PointsContract {
    pub fn output_id(&self) -> String {
        format!("points-{}", manifest_hash(self))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Points {
    /// Metres in the camera frame the contract names.
    pub points: Vec<[f64; 3]>,
    /// sRGB per point, when the camera gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<Vec<[u8; 3]>>,
}
impl ContractType for Points {
    const DATATYPE: &'static str = POINTS_SCHEMA;
}
impl Points {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.points.is_empty() && self.points.len() <= MAX_POINTS, "Depth capture needs 1–{MAX_POINTS} points");
        ensure!(self.points.iter().flatten().all(|v| v.is_finite() && v.abs() <= 100.), "Invalid depth point");
        ensure!(self.colors.as_ref().is_none_or(|c| c.len() == self.points.len()), "Depth color/point count differs");
        Ok(())
    }
}
