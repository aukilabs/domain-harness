use anyhow::Result;
use auki_components::ComponentRuntime;
use spatial_home::local_mapper::*;
use std::collections::BTreeMap;

fn map() -> Result<LocalMap> {
    LocalMap::new_for_robot(
        &ComponentRuntime::new("robot"),
        "robot",
        &uuid::Uuid::new_v4().to_string(),
        BRACKETBOT_FAMILY,
        spatial_home::rgbd::BRACKETBOT_FRAME,
        serde_json::from_str(include_str!("../config/portals.json"))?,
    )
}

#[test]
fn joints_are_published_next_to_the_pose_for_the_viewer_model() -> Result<()> {
    let mut map = map()?;
    assert!(map.metadata()["local_mapping"]["robot_joints"].is_null());
    // The adapter's JSON line, as the runner deserializes it.
    let event: SensorEvent = serde_json::from_str(
        r#"{"kind":"joints","timestamp_ns":7,"positions":{"lj0":-0.414,"rj5":1.553}}"#,
    )?;
    map.accept(event)?;
    let joints = &map.metadata()["local_mapping"]["robot_joints"];
    assert_eq!(joints["capture_timestamp_ns"], 7);
    assert_eq!(joints["positions"]["lj0"], -0.414);
    assert_eq!(joints["positions"]["rj5"], 1.553);
    // Joints are presentation only: they neither start mapping nor need odometry.
    assert!(map.latest_pose.is_none());
    Ok(())
}

#[test]
fn non_finite_joint_positions_are_rejected() -> Result<()> {
    let mut map = map()?;
    let positions = BTreeMap::from([("lj1".to_string(), f64::NAN)]);
    assert!(map.accept(SensorEvent::Joints { timestamp_ns: 1, positions }).is_err());
    assert!(map.latest_joints.is_none());
    Ok(())
}

#[test]
fn joint_health_and_battery_are_published_for_the_viewer() -> Result<()> {
    let mut map = map()?;
    assert!(map.metadata()["local_mapping"]["robot_health"].is_null());
    // The adapter's JSON line, as the runner deserializes it.
    let event: SensorEvent = serde_json::from_str(
        r#"{"kind":"health","timestamp_ns":9,"joints":{"lj0":{"temperature_c":45.0,"current_a":2.04}},"battery":{"voltage":18.48,"percent":32.2}}"#,
    )?;
    map.accept(event)?;
    let health = &map.metadata()["local_mapping"]["robot_health"];
    assert_eq!(health["joints"]["lj0"]["temperature_c"], 45.0);
    assert_eq!(health["battery"]["voltage"], 18.48);
    let bad: SensorEvent = serde_json::from_str(
        r#"{"kind":"health","timestamp_ns":10,"joints":{},"battery":{"voltage":18.0,"percent":-5.0}}"#,
    )?;
    assert!(map.accept(bad).is_err());
    Ok(())
}
