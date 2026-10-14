//! Peer-independent publication switch contract. Authorization belongs to the publisher.
use auki_components::ContractType;
use serde::{Deserialize, Serialize};
pub const OPERATION: &str = "set_enabled";
pub const STATE_SCHEMA: &str = "spatial-harness.component-enabled-state/v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetEnabled { pub enabled: bool }
impl ContractType for SetEnabled {
    const DATATYPE: &'static str = "spatial-harness.set-enabled/v1";
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State { pub enabled: bool }
impl ContractType for State { const DATATYPE: &'static str = STATE_SCHEMA; }
