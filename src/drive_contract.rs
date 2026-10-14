//! Home's pushed drive commands, shared by Home and every robot that follows
//! them (the full runner and the diagnostic peer) so both decode one contract.
use auki_components::ContractType;
use serde::{Deserialize, Serialize};

/// What Home pushes to robots the moment a phone's command is accepted.
/// Polling `snapshot` costs a full request round trip per read, which on a
/// loaded robot measured 130-390 ms; a pushed message is one hop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pushed {
    pub driver_peer_id: String,
    pub target_peer_id: String,
    pub session: String,
    pub sequence: u64,
    pub forward: f32,
    pub turn: f32,
}
impl ContractType for Pushed {
    const DATATYPE: &'static str = STREAM_SCHEMA;
}
pub const STREAM_SCHEMA: &str = "spatial.phone-drive-stream/v1";
pub fn stream_product(scene_id: &str) -> String {
    format!("local-scene:{scene_id}:drive")
}
pub fn stream_limits() -> auki_components::BufferLimits {
    auki_components::BufferLimits {
        max_entries: Some(4),
        max_bytes: Some(16 * 1024),
        target_duration: None,
    }
}
pub fn pushed_size(value: &Pushed) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |v| v.len())
}
