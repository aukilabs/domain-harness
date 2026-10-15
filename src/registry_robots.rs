//! Robots that declare their body through the registry (`robot_state`), for
//! phones. The viewer draws such a robot at its registry-backed pose with its
//! latest joints and health; phones only read the robot presence Home publishes
//! in each local scene. This copies each declared robot's scene pose, joints and
//! health into that presence, marked `registry`, using the same pose resolver as
//! the viewer. The pose is the body's base frame (its URDF root link) in the
//! scene frame, not `robot.base`.
use crate::local_scenes::LocalScenes;
use crate::robot_states::RobotStates;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

const REFRESH: Duration = Duration::from_millis(500);
const POSE_WAIT: Duration = Duration::from_secs(3);

/// Registry robots per local scene frame, as robot presence entries.
pub type Entries = std::sync::Mutex<BTreeMap<String, Vec<Value>>>;

pub async fn serve(
    scenes: Arc<LocalScenes>,
    states: RobotStates,
    network: watch::Receiver<Value>,
    previews: mpsc::Sender<crate::sensor_preview::Request>,
    stop: CancellationToken,
) {
    let mut tick = tokio::time::interval(REFRESH);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! { _ = stop.cancelled() => break, _ = tick.tick() => {} }
        let robots = states.view()["robots"].as_array().cloned().unwrap_or_default();
        let network = network.borrow().clone();
        let frames: Vec<String> = scenes.snapshots.keys()
            .filter_map(|id| scenes.view(id).ok())
            .filter_map(|v| v["frame"].as_str().map(String::from))
            .collect();
        let mut by_frame = BTreeMap::new();
        for frame in frames {
            let mut entries = Vec::new();
            for robot in &robots {
                if robot["joints"].is_null() && robot["health"].is_null() {
                    continue;
                }
                let Some(peer) = robot["peer_id"].as_str() else { continue };
                let read = crate::peer_locations::read_peer(&previews, &network, peer, &frame);
                let Ok(Ok(value)) = tokio::time::timeout(POSE_WAIT, read).await else { continue };
                if let Some(entry) = entry(robot, &value, &network, &frame) {
                    entries.push(entry);
                }
            }
            by_frame.insert(frame, entries);
        }
        if let Ok(mut shared) = scenes.registry_robots.lock() {
            *shared = by_frame;
        }
    }
}

/// One robot presence entry from a declared body and its resolved scene pose,
/// or None when the pose is not this body's base frame in `frame`.
pub fn entry(robot: &Value, value: &Value, network: &Value, frame: &str) -> Option<Value> {
    let peer = robot["peer_id"].as_str()?;
    // The pose must be for the declared body's base frame, as the viewer matches it.
    if value["resolved"]["references"]["from_frame"] != robot["base_frame"] {
        return None;
    }
    let home = &value["home_pose"];
    if home["frame"] != frame || home["tracking"]["status"] != "valid" || value["stream"] != "live" {
        return None;
    }
    let pose = &home["tracking"]["pose"];
    let translation: [f64; 3] = serde_json::from_value(pose["translation"].clone()).ok()?;
    let [x, y, z, w]: [f64; 4] = serde_json::from_value(pose["rotation_xyzw"].clone()).ok()?;
    if !translation.iter().chain(&[x, y, z, w]).all(|v| v.is_finite()) {
        return None;
    }
    let reported = network["peers"].as_array()
        .and_then(|p| p.iter().find(|p| p["peer_id"] == peer))
        .and_then(|p| p["name"].as_str())
        .unwrap_or(peer);
    let state = |kind: &str, fields: &[&str]| -> Value {
        let latest = &robot[kind];
        let Some(timestamp) = latest["timestamp_ns"].as_str().and_then(|t| t.parse::<u64>().ok()) else {
            return Value::Null;
        };
        let mut value = json!({"timestamp_ns": timestamp});
        for field in fields {
            if !latest["state"][field].is_null() {
                value[*field] = latest["state"][field].clone();
            }
        }
        value
    };
    Some(json!({
        "peer_id": peer,
        "name": crate::peer_aliases::name(peer, reported),
        "registry": true,
        "pose": {"from_frame_id": robot["base_frame"]["id"], "to_frame_id": frame,
                 "translation": translation, "rotation_wxyz": [w, x, y, z]},
        "joints": state("joints", &["positions"]),
        "health": state("health", &["joints", "battery"]),
        "received_age_ms": value["age_ms"],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> (Value, Value) {
        let base = json!({"peer_id": "robot", "id": "bracketbot-root", "hash": "h1"});
        let robot = json!({"peer_id": "robot", "model": "bracketbot", "base_frame": base,
            "joints": {"timestamp_ns": "7", "age_ms": 5, "state": {"positions": {"rj1": 0.4}}},
            "health": {"timestamp_ns": "8", "age_ms": 5, "state": {
                "joints": {"rj1": {"temperature_c": 47.0, "current_a": 1.2}},
                "battery": {"voltage": 24.1, "percent": 61.0}}},
            "pointing": null});
        let value = json!({"stream": "live", "age_ms": 40,
            "resolved": {"references": {"from_frame": base}},
            "home_pose": {"frame": "local-scene:lab:origin", "tracking": {"status": "valid",
                "pose": {"translation": [1.0, 0.0, 2.0], "rotation_xyzw": [0.0, 0.0, 0.0, 1.0]}}}});
        (robot, value)
    }

    #[test]
    fn declared_robot_becomes_a_robot_presence_entry() {
        let (robot, value) = fixtures();
        let network = json!({"peers": [{"peer_id": "robot", "name": "Bracketbot 0184"}]});
        let entry = entry(&robot, &value, &network, "local-scene:lab:origin").unwrap();
        assert_eq!(entry["registry"], true);
        assert_eq!(entry["name"], "Bracketbot 0184");
        assert_eq!(entry["pose"]["to_frame_id"], "local-scene:lab:origin");
        assert_eq!(entry["pose"]["from_frame_id"], "bracketbot-root");
        assert_eq!(entry["pose"]["rotation_wxyz"], json!([1.0, 0.0, 0.0, 0.0]));
        assert_eq!(entry["joints"], json!({"timestamp_ns": 7, "positions": {"rj1": 0.4}}));
        assert_eq!(entry["health"]["timestamp_ns"], 8);
        assert_eq!(entry["health"]["battery"]["percent"], 61.0);
        assert_eq!(entry["health"]["joints"]["rj1"]["temperature_c"], 47.0);
        // Phones decode presence labels through the same scene view as before.
        let mut view = json!({"notes": []});
        crate::robot_presence::append_labels(&mut view,
            &json!({"frame": "local-scene:lab:origin", "poses": [entry]}), "local-scene:lab:origin");
        assert_eq!(view["notes"][0]["robot_pose"]["registry"], true);
    }

    #[test]
    fn other_frames_stale_streams_and_other_bodies_are_skipped() {
        let (robot, value) = fixtures();
        let network = json!({"peers": []});
        assert!(entry(&robot, &value, &network, "local-scene:other:origin").is_none());
        let mut stale = value.clone();
        stale["stream"] = json!("stale");
        assert!(entry(&robot, &stale, &network, "local-scene:lab:origin").is_none());
        let mut lost = value.clone();
        lost["home_pose"]["tracking"]["status"] = json!("lost");
        assert!(entry(&robot, &lost, &network, "local-scene:lab:origin").is_none());
        let mut other = value.clone();
        other["resolved"]["references"]["from_frame"]["hash"] = json!("h2");
        assert!(entry(&robot, &other, &network, "local-scene:lab:origin").is_none());
        // Without a reported name, the peer ID stands in.
        assert_eq!(entry(&robot, &value, &network, "local-scene:lab:origin").unwrap()["name"], "robot");
    }
}
