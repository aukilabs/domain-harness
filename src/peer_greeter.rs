//! Home's `peer-greeter` component: `greet_peer` sends a robot to a peer and
//! greets them (see `greet`). The operable only checks and queues the request;
//! Home's greeting job follows the peer's live pose, re-sends the robot's
//! `navigate` as they move, stops within reach and calls the robot's `greet`.
use crate::greet::{GreetPeer, GreetPeerAccepted, PEER_COMPONENT, PEER_OPERATION, peer_contract};
use anyhow::Result;
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;

/// A queued `greet_peer`, with who asked.
pub struct Request {
    pub instruction: GreetPeer,
    pub caller: String,
}

pub fn mount(runtime: &ComponentRuntime, endpoint: &ComponentProtocolEndpoint,
    requests: tokio::sync::mpsc::Sender<Request>) -> Result<Component> {
    let component = runtime.component(ComponentSpec::new(PEER_COMPONENT).operable(peer_contract()))?;
    let operation = component.operable(
        PEER_OPERATION,
        |ctx| !ctx.caller_peer_id.is_empty(),
        move |ctx, instruction: GreetPeer| {
            let rejected = |e: &str| InvocationError::Rejected(e.into());
            if instruction.request_id.is_empty() || instruction.request_id.len() > 128 { return Err(rejected("Invalid request ID")); }
            if instruction.peer.is_empty() || instruction.peer.len() > 128 { return Err(rejected("Name the peer to greet by peer ID")); }
            let accepted = GreetPeerAccepted {
                request_id: instruction.request_id.clone(),
                robot_peer_id: instruction.robot.clone().unwrap_or_default(),
                peer_id: instruction.peer.clone(),
                summary: "Home is sending a robot to greet them".into(),
            };
            requests.try_send(Request { instruction, caller: ctx.caller_peer_id.clone() })
                .map_err(|_| InvocationError::TargetUnavailable)?;
            Ok(accepted)
        },
    )?;
    component.expose()?;
    endpoint.export_operable(&operation)?;
    Ok(component)
}
