//! Greeting contracts, shared by Home and every robot that offers them.
//!
//! - A robot's `greeter` component offers `greet`: say hello, now, where it stands.
//! - Home's `peer-greeter` component offers `greet_peer`: send a robot to a peer
//!   in the scene, following the peer as they move, and greet them within reach.
use anyhow::{Result, ensure};
use auki_components::{ContractType, Exposure, OperableContract};
use serde::{Deserialize, Serialize};

pub const COMPONENT: &str = "greeter";
pub const OPERATION: &str = "greet";
pub const INSTRUCTION: &str = "spatial-harness.greet/v1";
pub const RESULT: &str = "spatial-harness.greet-accepted/v1";

pub const PEER_COMPONENT: &str = "peer-greeter";
pub const PEER_OPERATION: &str = "greet_peer";
pub const PEER_INSTRUCTION: &str = "spatial-harness.greet-peer/v1";
pub const PEER_RESULT: &str = "spatial-harness.greet-peer-accepted/v1";

/// How close the robot comes before it stops and greets.
pub const REACH_M: f64 = 1.0;

pub fn contract() -> OperableContract {
    OperableContract { name: OPERATION.into(), instruction: INSTRUCTION.into(), result: RESULT.into(), exposure: Exposure::Cluster }
}
pub fn peer_contract() -> OperableContract {
    OperableContract { name: PEER_OPERATION.into(), instruction: PEER_INSTRUCTION.into(), result: PEER_RESULT.into(), exposure: Exposure::Cluster }
}

/// A name the robot may say: letters, digits, spaces and a few marks, at most 40
/// characters. Anything else is dropped rather than read out.
pub fn speakable(name: &str) -> String {
    let kept: String = name.chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '\'' | '.'))
        .take(40).collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Greet {
    pub request_id: String,
    /// Who to greet by name; without one the robot just says hello.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
impl ContractType for Greet {
    const DATATYPE: &'static str = INSTRUCTION;
}
impl Greet {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.request_id.is_empty() && self.request_id.len() <= 128, "Invalid greeting request ID");
        ensure!(self.name.as_ref().is_none_or(|n| n.chars().count() <= 64), "Name too long");
        Ok(())
    }
    /// What the robot says.
    pub fn text(&self) -> String {
        match self.name.as_deref().map(speakable).filter(|n| !n.is_empty()) {
            Some(name) => format!("Hi {name}! Nice to meet you."),
            None => "Hi there! Nice to meet you.".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GreetAccepted {
    pub request_id: String,
    /// Whether the robot has a voice to say it with.
    pub speaks: bool,
    pub text: String,
}
impl ContractType for GreetAccepted {
    const DATATYPE: &'static str = RESULT;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GreetPeer {
    pub request_id: String,
    /// The robot, by peer ID or Home alias; may be left out when only one can greet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub robot: Option<String>,
    /// The peer to go to and greet, by peer ID.
    pub peer: String,
}
impl ContractType for GreetPeer {
    const DATATYPE: &'static str = PEER_INSTRUCTION;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GreetPeerAccepted {
    pub request_id: String,
    pub robot_peer_id: String,
    pub peer_id: String,
    pub summary: String,
}
impl ContractType for GreetPeerAccepted {
    const DATATYPE: &'static str = PEER_RESULT;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spoken_names_are_plain_and_short() {
        assert_eq!(speakable("  Arshak  "), "Arshak");
        assert_eq!(speakable("Bob; rm -rf /"), "Bob rm -rf");
        assert_eq!(speakable(&"a".repeat(90)).len(), 40);
        let hi = |name: Option<&str>| Greet { request_id: "r".into(), name: name.map(Into::into) }.text();
        assert_eq!(hi(Some("Arshak")), "Hi Arshak! Nice to meet you.");
        assert_eq!(hi(Some("$$$")), "Hi there! Nice to meet you.");
        assert_eq!(hi(None), "Hi there! Nice to meet you.");
    }
}
