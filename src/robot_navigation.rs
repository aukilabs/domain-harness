//! Home's caller for the `navigate` Operable robots advertise (see `navigate`).
//!
//! Home plans a route on its scene map, then hands the robot the route's
//! waypoints in Home's scene frame, named by full registry reference. The robot
//! resolves them through its own published transforms and its own navigation
//! stack drives. Discovery is the same Catalog scan as `robot_pointing`.
use crate::navigate::{self, Cancel, Cancelled, Navigate, NavigateAccepted};
use crate::robot_pointing::{Candidate, select_for};
use anyhow::{Result, anyhow, ensure};
use serde_json::Value;
use std::time::Duration;

pub fn select(network: &Value, robot: Option<&str>) -> Result<Candidate> {
    select_for(network, robot, &navigate::contract(), "navigate")
}

/// Route points to waypoints: drop the start (where the robot stands) and
/// points closer than `gap_m` to the last kept one; the goal is always kept.
pub fn waypoints(points: &[[f64; 3]], gap_m: f64) -> Result<Vec<[f64; 3]>> {
    ensure!(points.len() >= 2, "The route has no way to follow");
    let distance = |a: [f64; 3], b: [f64; 3]| (a[0] - b[0]).hypot(a[2] - b[2]).hypot(a[1] - b[1]);
    let mut kept: Vec<[f64; 3]> = Vec::new();
    let mut last = points[0];
    for &point in &points[1..points.len() - 1] {
        if distance(point, last) >= gap_m {
            kept.push(point);
            last = point;
        }
    }
    let goal = points[points.len() - 1];
    if kept.last().is_some_and(|k| distance(*k, goal) < gap_m) {
        kept.pop();
    }
    kept.push(goal);
    ensure!(kept.len() <= navigate::MAX_WAYPOINTS, "The route is too long to send at once");
    Ok(kept)
}

pub enum Request {
    Navigate(Navigate, tokio::sync::oneshot::Sender<Result<NavigateAccepted>>),
    Cancel(Cancel, tokio::sync::oneshot::Sender<Result<Cancelled>>),
    /// A robot's `greet` (see `greet`): said where it stands, nothing moves.
    Greet(crate::greet::Greet, tokio::sync::oneshot::Sender<Result<crate::greet::GreetAccepted>>),
}
/// A Home-side invocation, served by the task that owns Home's peer.
pub struct Invoke {
    pub candidate: Candidate,
    pub request: Request,
}

pub async fn serve(
    peer: &auki_sdk::AukiPeer,
    mut requests: tokio::sync::mpsc::Receiver<Invoke>,
    stop: &tokio_util::sync::CancellationToken,
) {
    let client = auki_component_protocol::ComponentProtocolClient::new(peer.protocols());
    loop {
        let request = tokio::select! { _ = stop.cancelled() => break, r = requests.recv() => r };
        let Some(Invoke { candidate, request }) = request else { break };
        let client = client.clone();
        tokio::spawn(async move {
            let target = || -> Result<(auki_sdk::PeerId, auki_sdk::Multiaddr)> {
                Ok((
                    candidate.peer_id.parse()?,
                    candidate.route.clone()
                        .ok_or_else(|| anyhow!("Robot catalog route unknown; wait for its Catalog refresh"))?
                        .parse()?,
                ))
            };
            let deadline = Some(Duration::from_secs(8));
            match request {
                Request::Navigate(instruction, reply) => {
                    let work = async {
                        let (expected, route) = target()?;
                        let accepted = client.invoke_exact::<_, NavigateAccepted>(expected, route, candidate.component.clone(),
                            navigate::OPERATION, "home-navigation", uuid::Uuid::new_v4().to_string(), deadline, &instruction)
                            .await?.result;
                        ensure!(accepted.request_id == instruction.request_id && accepted.frame == instruction.frame,
                            "Robot navigation result does not match the request");
                        Ok(accepted)
                    };
                    let result = tokio::time::timeout(Duration::from_secs(10), work).await
                        .unwrap_or_else(|_| Err(anyhow!("Robot navigation request timed out")));
                    let _ = reply.send(result);
                }
                Request::Greet(instruction, reply) => {
                    let work = async {
                        let (expected, route) = target()?;
                        let accepted = client.invoke_exact::<_, crate::greet::GreetAccepted>(expected, route, candidate.component.clone(),
                            crate::greet::OPERATION, "home-greeting", uuid::Uuid::new_v4().to_string(), deadline, &instruction)
                            .await?.result;
                        ensure!(accepted.request_id == instruction.request_id, "Robot greeting result does not match the request");
                        Ok(accepted)
                    };
                    let result = tokio::time::timeout(Duration::from_secs(10), work).await
                        .unwrap_or_else(|_| Err(anyhow!("Robot greeting request timed out")));
                    let _ = reply.send(result);
                }
                Request::Cancel(instruction, reply) => {
                    let work = async {
                        let (expected, route) = target()?;
                        Ok(client.invoke_exact::<_, Cancelled>(expected, route, candidate.component.clone(),
                            navigate::CANCEL, "home-navigation", uuid::Uuid::new_v4().to_string(), deadline, &instruction)
                            .await?.result)
                    };
                    let result = tokio::time::timeout(Duration::from_secs(10), work).await
                        .unwrap_or_else(|_| Err(anyhow!("Robot stop request timed out")));
                    let _ = reply.send(result);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_become_thinned_waypoints_that_end_at_the_goal() {
        let route = [[0., 0., 0.], [0.1, 0., 0.], [0.3, 0., 0.], [0.6, 0., 0.], [0.7, 0., 0.]];
        let kept = waypoints(&route, 0.25).unwrap();
        assert_eq!(kept.first(), Some(&[0.3, 0., 0.]), "the start is where the robot stands");
        assert_eq!(kept.last(), Some(&[0.7, 0., 0.]));
        assert!(!kept.contains(&[0.6, 0., 0.]), "a point too close to the goal yields to it");
        assert!(waypoints(&[[0., 0., 0.]], 0.25).is_err());
        assert_eq!(waypoints(&[[0., 0., 0.], [1., 0., 0.]], 0.25).unwrap(), vec![[1., 0., 0.]]);
    }
}
