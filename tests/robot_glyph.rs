use anyhow::Result;
use auki_components::ComponentRuntime;
use auki_odometry::Pose;
use auki_registry::{CoordinateConvention, FrameRegistryEntry};
use serde_json::{Value, json};
use spatial_home::{local_mapper::*, network::CanonicalScene, scene_mirror::render_scene};

fn scene(family: &str, yaw: f64) -> Result<CanonicalScene> {
    let runtime = ComponentRuntime::new("robot");
    let mut map = LocalMap::new_for_robot(
        &runtime,
        "robot",
        &uuid::Uuid::new_v4().to_string(),
        family,
        spatial_home::rgbd::BRACKETBOT_FRAME,
        serde_json::from_str(include_str!("../config/portals.json"))?,
    )?;
    map.accept(SensorEvent::Odometry {
        timestamp_ns: 1,
        pose: identity(),
    })?;
    map.accept(SensorEvent::Odometry {
        timestamp_ns: 2,
        pose: Pose {
            translation: [1., 2., 3.],
            rotation_xyzw: [0., 0., (yaw / 2.).sin(), (yaw / 2.).cos()],
        },
    })?;
    Ok(CanonicalScene {
        schema: spatial_home::network::SCENE_SCHEMA.into(),
        sdk_revision: spatial_home::model::SDK_REV.into(),
        authority_peer_id: "robot".into(),
        frame: map.frame.clone(),
        publication_clock: spatial_home::model::clock_entry("robot", "publication"),
        portals: map.portals()?,
        voxels: map.voxels.clone().unwrap(),
        objects: vec![],
        registries: map.metadata(),
    })
}

fn direction(pose: &Value, axis: [f64; 3]) -> Result<[f64; 3]> {
    let pose: Pose = serde_json::from_value(pose.clone())?;
    let point = spatial_home::runner_pose::compose(
        &pose,
        &Pose {
            translation: axis,
            ..identity()
        },
    );
    Ok(std::array::from_fn(|i| {
        point.translation[i] - pose.translation[i]
    }))
}

fn close(actual: [f64; 3], expected: [f64; 3]) {
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(a, b)| (a - b).abs() < 1e-9),
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn glyph_forward_follows_declared_body_axes_through_yaw() -> Result<()> {
    for (family, yaw, forward) in [
        (BRACKETBOT_FAMILY, 0., [0., 0., -1.]),
        (
            BRACKETBOT_FAMILY,
            std::f64::consts::FRAC_PI_2,
            [-1., 0., 0.],
        ),
        ("galbot-local", 0., [1., 0., 0.]),
        ("galbot-local", std::f64::consts::FRAC_PI_2, [0., 0., -1.]),
    ] {
        let scene = scene(family, yaw)?;
        let before = serde_json::to_value(&scene)?;
        let view = render_scene(&scene)?;
        let robot = &view["local_mapping"]["robot_pose"];
        close(direction(&robot["glyph_pose"], [1., 0., 0.])?, forward);
        close(direction(&robot["glyph_pose"], [0., 0., 1.])?, [0., 1., 0.]);
        close(
            serde_json::from_value(robot["glyph_pose"]["translation"].clone())?,
            [1., 3., -2.],
        );
        assert_eq!(
            robot["pose"]["translation"],
            robot["glyph_pose"]["translation"]
        );
        assert_eq!(
            robot["from_frame"],
            before["registries"]["local_mapping"]["robot_pose"]["from_frame"]
        );
        assert_eq!(
            robot["glyph_frame"],
            json!(FrameRegistryEntry::ros_body("home", "viewer.robot-glyph"))
        );
        let body: Pose = serde_json::from_value(
            before["registries"]["local_mapping"]["robot_pose"]["pose"].clone(),
        )?;
        let target = spatial_home::conventions::registry("home", view["frame"].as_str().unwrap())?;
        assert_eq!(
            robot["pose"],
            json!(spatial_home::conventions::pose(
                &body,
                &scene.voxels.definition.frame,
                &target
            )?)
        );
        assert_eq!(
            serde_json::to_value(&scene)?,
            before,
            "rendering must not mutate the physical scene"
        );
    }
    Ok(())
}

#[test]
fn existing_runner_registry_supplies_body_axes_without_redeployment() -> Result<()> {
    let mut scene = scene(BRACKETBOT_FAMILY, 0.)?;
    let explicit = render_scene(&scene)?;
    scene.registries["local_mapping"]["robot_pose"]
        .as_object_mut()
        .unwrap()
        .remove("from_frame");
    assert_eq!(render_scene(&scene)?, explicit);
    Ok(())
}

#[test]
fn shared_map_resolves_each_peers_body_axes_independently() -> Result<()> {
    let mut scene = scene(BRACKETBOT_FAMILY, 0.)?;
    let mut bracketbot = scene.registries["local_mapping"]["robot_pose"].clone();
    bracketbot.as_object_mut().unwrap().remove("from_frame");
    let mut ros = bracketbot.clone();
    ros["clock"]["peer_id"] = json!("other");
    scene.registries["input_registries"] = json!({
        "robot":{"frames":[FrameRegistryEntry::in_convention("robot", "robot.base", CoordinateConvention::ZUpRightForward)]},
        "other":{"frames":[FrameRegistryEntry::ros_body("other", "robot.base")]}
    });
    scene.registries["frames"] = json!([scene.voxels.definition.frame]);
    scene.registries["local_mapping"] = json!({"robot_pose":bracketbot,"runners":{
        "robot":{"status":"aligned","robot_pose":bracketbot},
        "other":{"status":"aligned","robot_pose":ros}
    }});
    let view = render_scene(&scene)?;
    let runners = &view["local_mapping"]["runners"];
    close(
        direction(&runners["robot"]["robot_pose"]["glyph_pose"], [1., 0., 0.])?,
        [0., 0., -1.],
    );
    close(
        direction(&runners["other"]["robot_pose"]["glyph_pose"], [1., 0., 0.])?,
        [1., 0., 0.],
    );
    assert_eq!(
        view["local_mapping"]["robot_pose"],
        runners["robot"]["robot_pose"]
    );
    Ok(())
}

#[test]
fn explicit_source_frame_controls_glyph_and_invalid_units_are_rejected() -> Result<()> {
    let mut scene = scene(BRACKETBOT_FAMILY, 0.)?;
    // The declaration, not the robot family or a hard-coded heading, is authoritative.
    scene.registries["local_mapping"]["robot_pose"]["from_frame"] =
        json!(FrameRegistryEntry::ros_body("robot", "custom-body"));
    let view = render_scene(&scene)?;
    close(
        direction(
            &view["local_mapping"]["robot_pose"]["glyph_pose"],
            [1., 0., 0.],
        )?,
        [1., 0., 0.],
    );
    scene.registries["local_mapping"]["robot_pose"]["from_frame"]["units"] = json!("millimeters");
    assert!(render_scene(&scene).is_err());
    Ok(())
}
