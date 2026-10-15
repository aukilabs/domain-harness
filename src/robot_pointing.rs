//! Home's caller for the `point_at` Operable robots advertise (see `point_at`).
//!
//! Robots export `arm-pointer` in their Catalog; Home finds it in the peer
//! Catalogs it already polls and invokes it with a target in Home's scene
//! frame, named by full registry reference. The robot resolves that frame
//! through its own published transforms and reports whether its arm moves.
use crate::point_at::{self, PointAt, PointAtAccepted};
use anyhow::{Result, anyhow, ensure};
use auki_components::{CatalogComponentEntry, ComponentReference};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// One advertised `point_at` Operable with the route its Catalog came from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub peer_id: String,
    pub route: Option<String>,
    pub component: ComponentReference,
}

/// Peers whose Catalog advertises this exact pointing contract.
pub fn candidates(network: &Value) -> Vec<Candidate> {
    candidates_for(network, &point_at::contract())
}

/// Peers whose Catalog advertises exactly `contract` (any robot operation).
pub fn candidates_for(network: &Value, contract: &auki_components::OperableContract) -> Vec<Candidate> {
    let mut found = Vec::new();
    for row in network["peers"].as_array().into_iter().flatten() {
        let Some(entries) = row["catalog"]["components"].as_array() else { continue };
        for entry in entries {
            let Ok(entry) = serde_json::from_value::<CatalogComponentEntry>(entry.clone()) else { continue };
            let advertised = entry.manifest.operables.iter().any(|o| o == contract);
            if advertised && row["peer_id"] == entry.manifest.peer_id.as_str() {
                found.push(Candidate {
                    peer_id: entry.manifest.peer_id.clone(),
                    route: row["catalog_route"].as_str().map(str::to_owned),
                    component: entry.manifest.reference(),
                });
            }
        }
    }
    found
}

/// Resolve the robot by exact peer ID or Home alias/name; with none named, the only candidate.
pub fn select(network: &Value, robot: Option<&str>) -> Result<Candidate> {
    select_for(network, robot, &point_at::contract(), "point")
}

/// `select` for any robot operation; `verb` names what the robots can do in errors.
pub fn select_for(network: &Value, robot: Option<&str>, contract: &auki_components::OperableContract, verb: &str) -> Result<Candidate> {
    let all = candidates_for(network, contract);
    let names = |id: &str| {
        network["peers"].as_array().into_iter().flatten()
            .find(|p| p["peer_id"] == id)
            .map(|p| ["alias", "name", "reported_name"].iter()
                .filter_map(|k| p[*k].as_str()).map(str::to_lowercase).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let listed = || all.iter().map(|c| c.peer_id.clone()).collect::<Vec<_>>().join(", ");
    let operation = contract.name.as_str();
    let Some(robot) = robot.map(str::trim).filter(|r| !r.is_empty()) else {
        return match all.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(anyhow!("No peer advertises a {operation} operable")),
            _ => Err(anyhow!("Several robots can {verb} ({}); name one", listed())),
        };
    };
    let wanted = robot.to_lowercase();
    let matches = all.iter()
        .filter(|c| c.peer_id == robot || names(&c.peer_id).contains(&wanted))
        .cloned().collect::<Vec<_>>();
    match matches.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(anyhow!("{robot:?} does not advertise a {operation} operable. Robots that can {verb}: {}",
            if all.is_empty() { "none".into() } else { listed() })),
        _ => Err(anyhow!("{robot:?} matches several robots; use an exact peer ID")),
    }
}

/// A Home-side invocation, served by the task that owns Home's peer.
pub struct Invoke {
    pub candidate: Candidate,
    pub instruction: PointAt,
    pub reply: tokio::sync::oneshot::Sender<Result<PointAtAccepted>>,
}

pub async fn serve(
    peer: &auki_sdk::AukiPeer,
    mut requests: tokio::sync::mpsc::Receiver<Invoke>,
    stop: &tokio_util::sync::CancellationToken,
) {
    let client = auki_component_protocol::ComponentProtocolClient::new(peer.protocols());
    loop {
        let request = tokio::select! { _ = stop.cancelled() => break, r = requests.recv() => r };
        let Some(Invoke { candidate, instruction, reply }) = request else { break };
        let client = client.clone();
        tokio::spawn(async move {
            let work = async {
                let expected: auki_sdk::PeerId = candidate.peer_id.parse()?;
                let route: auki_sdk::Multiaddr = candidate.route
                    .ok_or_else(|| anyhow!("Robot catalog route unknown; wait for its Catalog refresh"))?
                    .parse()?;
                let accepted = client.invoke_exact::<_, PointAtAccepted>(expected, route, candidate.component,
                    point_at::OPERATION, "home-chat", uuid::Uuid::new_v4().to_string(),
                    Some(Duration::from_secs(5)), &instruction).await?.result;
                ensure!(accepted.request_id == instruction.request_id && accepted.frame == instruction.frame,
                    "Robot pointing result does not match the request");
                Ok(accepted)
            };
            let result = tokio::time::timeout(Duration::from_secs(8), work).await
                .unwrap_or_else(|_| Err(anyhow!("Robot pointing request timed out")));
            let _ = reply.send(result);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auki_components::OperableContract;
    use serde_json::json;

    const ROBOT: &str = "12D3KooWRobot";

    #[test]
    fn discovers_and_selects_advertised_pointing_robots() {
        let manifest = |peer: &str, operables: Vec<OperableContract>| json!({"manifest":{
            "schema":"auki.component-manifest/v1","peer_id":peer,"component_id":point_at::COMPONENT,
            "product_inputs":[],"observables":[],"operables":operables},
            "manifest_hash":"h","current_product_inputs":{},"current_outputs":{}});
        let mut other = point_at::contract();
        other.instruction = "spatial.other/v1".into();
        let network = json!({"peers":[
            {"peer_id":ROBOT,"alias":"broomboy","catalog_route":"/ip4/1.2.3.4/tcp/1",
             "catalog":{"components":[manifest(ROBOT, vec![point_at::contract()])]}},
            {"peer_id":"12D3KooWPhone","catalog":{"components":[manifest("12D3KooWPhone", vec![other])]}}]});
        // The fixture must match the SDK schema, or discovery would pass vacuously.
        serde_json::from_value::<CatalogComponentEntry>(manifest(ROBOT, vec![point_at::contract()])).unwrap();
        assert_eq!(candidates(&network).len(), 1);
        assert_eq!(select(&network, None).unwrap().peer_id, ROBOT);
        assert_eq!(select(&network, Some("Broomboy")).unwrap().peer_id, ROBOT);
        assert!(select(&network, Some("12D3KooWPhone")).unwrap_err().to_string().contains("does not advertise"));
    }
}
