use anyhow::Result;
use auki_components::{ClockReference, ProductReference};
use auki_odometry::Pose;
use auki_registry::FrameRegistryEntry;
use auki_scenegraph::{MapSnapshot, component::SnapshotReference};
use auki_voxel_map::{DepthObservation, SensorBinding, TimedSensorPose};
use prost::Message;
use spatial_home::{
    collaborative_mapper::{CollaborativeMap, LocalizedScan, reference_in_frame},
    local_mapper::{canonical_origin, compose, identity, inverse},
};
fn canonical() -> Result<MapSnapshot> {
    let dir = tempfile::tempdir()?;
    Ok(
        spatial_home::model::Store::open_canonical(dir.path().join("portals.json"))?
            .built
            .portals,
    )
}
fn clock(peer: &str) -> ClockReference {
    spatial_home::model::clock_ref(&spatial_home::model::clock_entry(peer, "session"))
}
fn rigid(p: &Pose, from: &str, to: &str) -> auki_scenegraph::RigidTransform {
    let [x, y, z, w] = p.rotation_xyzw;
    auki_scenegraph::RigidTransform {
        from_frame_id: from.into(),
        to_frame_id: to.into(),
        translation: p.translation,
        rotation_wxyz: [w, x, y, z],
    }
}
fn pose(r: &auki_scenegraph::RigidTransform) -> Pose {
    let [w, x, y, z] = r.rotation_wxyz;
    Pose {
        translation: r.translation,
        rotation_xyzw: [x, y, z, w],
    }
}
fn scan(
    canonical: &MapSnapshot,
    peer: &str,
    marker: &str,
    seq: u64,
    t: u64,
    hit: f64,
    rgb: [u8; 3],
) -> Result<LocalizedScan> {
    let frame = format!("home-live:{}:zup", uuid::Uuid::new_v4());
    let reference = reference_in_frame(canonical, &frame)?;
    let marker_shared = pose(&reference.scenegraph.anchors[marker].pose_in_map);
    // Two unrelated odometry origins and yaw rotations; they see different markers.
    let a = if peer == "a" { 0.7_f64 } else { -1.2 };
    let odometry_shared = Pose {
        translation: if peer == "a" {
            [4., -2., 0.3]
        } else {
            [-8., 1., -0.4]
        },
        rotation_xyzw: [0., 0., (a / 2.).sin(), (a / 2.).cos()],
    };
    let marker_odom = compose(&inverse(&odometry_shared)?, &marker_shared)?;
    let origin = canonical_origin(
        1,
        clock(peer),
        &marker_odom,
        &reference.scenegraph.anchors[marker],
    )?;
    for (actual, expected) in origin
        .odometry_to_local
        .translation
        .iter()
        .zip(odometry_shared.translation)
    {
        assert!((actual - expected).abs() < 1e-8);
    }
    let sensor_shared = Pose {
        translation: if peer == "a" {
            [0.05, 0.05, 0.05]
        } else {
            [-0.05, 0.05, 0.05]
        },
        ..identity()
    };
    let sensor_odom = compose(&inverse(&odometry_shared)?, &sensor_shared)?;
    let actual_sensor = compose(&origin.odometry_to_local, &sensor_odom)?;
    let source = ProductReference {
        peer_id: peer.into(),
        product_id: "depth".into(),
        manifest_hash: format!("{peer}-manifest"),
    };
    let sensor_frame = FrameRegistryEntry::ros_body(peer, "sensor");
    let observation = DepthObservation {
        source: source.clone(),
        sequence: seq,
        timestamp_ns: t,
        clock: clock(peer),
        sensor_frame_id: "sensor".into(),
        hit_points_m: vec![[hit - sensor_shared.translation[0], 0., 0.]],
        pose: TimedSensorPose {
            timestamp_ns: t,
            clock: clock(peer),
            sensor_to_map: rigid(&actual_sensor, "sensor", &frame),
            portal_snapshot: SnapshotReference {
                product: ProductReference {
                    peer_id: "home".into(),
                    product_id: "local-observed".into(),
                    manifest_hash: "snapshot".into(),
                },
                sequence: 1,
            },
        },
    };
    Ok(LocalizedScan {
        observation,
        binding: SensorBinding {
            sensor_product: source,
            sensor_frame,
            observation_clock: clock(peer),
        },
        colors: vec![Some(rgb)],
        origin,
    })
}
fn occupied_x(map: &CollaborativeMap) -> Vec<(i32, f64)> {
    let s = map.voxels.as_ref().unwrap();
    let u = auki_datatypes::map::MapUpdate::decode(s.checkpoint.as_slice()).unwrap();
    u.checkpoint
        .unwrap()
        .voxel_chunks
        .into_iter()
        .flat_map(|c| {
            c.voxels
                .into_iter()
                .filter(|v| v.occupancy_evidence > 0.)
                .map(move |v| (c.chunk_x * 16 + v.x as i32, v.occupancy_evidence))
        })
        .collect()
}
#[test]
fn different_markers_and_unsynchronized_clocks_fuse_and_clear_the_same_cells() -> Result<()> {
    let canonical = canonical()?;
    let unchanged = serde_json::to_value(&canonical)?;
    let mut map = CollaborativeMap::new("home", &uuid::Uuid::new_v4().to_string(), &canonical)?;
    let a = scan(
        &canonical,
        "a",
        "UCOF1J40N7O",
        1,
        9_000_000_000_000,
        0.55,
        [255, 0, 0],
    )?;
    let b = scan(&canonical, "b", "HRIPLL40GL6", 1, 100, 0.55, [0, 0, 255])?;
    assert_ne!(
        a.origin.calibration.as_ref().unwrap().portal_id,
        b.origin.calibration.as_ref().unwrap().portal_id
    );
    assert!(map.integrate(a.clone())?);
    assert!(map.integrate(b)?);
    assert_eq!(occupied_x(&map), vec![(5, 2.)]);
    let checkpoint =
        auki_datatypes::map::MapUpdate::decode(map.voxels.as_ref().unwrap().checkpoint.as_slice())?;
    let colors = checkpoint
        .checkpoint
        .unwrap()
        .voxel_chunks
        .into_iter()
        .flat_map(|c| c.voxels)
        .find(|v| v.occupancy_evidence > 0.)
        .unwrap()
        .color
        .unwrap();
    assert_eq!(colors.weight, 2.);
    assert_eq!(colors.red_sum, 1.);
    assert_eq!(colors.blue_sum, 1.);

    assert_eq!(map.voxels.as_ref().unwrap().definition.sources.len(), 2);
    let before = serde_json::to_value(map.scene()?)?;
    assert!(!map.integrate(a)?);
    assert_eq!(serde_json::to_value(map.scene()?)?, before);
    assert!(map.integrate(scan(
        &canonical,
        "a",
        "UCOF1J40N7O",
        2,
        9_000_000_000_001,
        1.05,
        [0, 255, 0]
    )?)?);
    assert!(map.integrate(scan(
        &canonical,
        "b",
        "HRIPLL40GL6",
        2,
        101,
        1.05,
        [0, 255, 0]
    )?)?);
    assert!(
        !occupied_x(&map).iter().any(|v| v.0 == 5),
        "free rays from both peers must clear the old surface"
    );
    let scene = map.scene()?.unwrap();
    scene.validate("home")?;
    assert_eq!(
        scene.registries["collaborative_mapping"]["contributors"],
        serde_json::json!({"a":2,"b":2})
    );
    assert_eq!(scene.voxels.latest_observation.unwrap().clock, clock("b"));
    assert_eq!(serde_json::to_value(&canonical)?, unchanged);
    Ok(())
}
#[test]
fn wrong_reference_unknown_portal_and_cross_clock_pose_are_rejected_atomically() -> Result<()> {
    let canonical = canonical()?;
    let mut map = CollaborativeMap::new("home", &uuid::Uuid::new_v4().to_string(), &canonical)?;
    let good = scan(&canonical, "b", "HRIPLL40GL6", 1, 100, 0.55, [0, 0, 255])?;
    let mut wrong = good.clone();
    wrong
        .origin
        .calibration
        .as_mut()
        .unwrap()
        .portal_pose_in_shared
        .translation[0] += 1.;
    assert!(map.integrate(wrong).is_err());
    assert!(map.voxels.is_none());
    let mut unknown = good.clone();
    unknown.origin.calibration.as_mut().unwrap().portal_id = "UNKNOWN".into();
    assert!(map.integrate(unknown).is_err());
    assert!(map.voxels.is_none());
    let mut other_clock = good.clone();
    other_clock.observation.pose.clock = clock("a");
    assert!(map.integrate(other_clock).is_err());
    assert!(map.voxels.is_none());
    assert!(map.integrate(good)?);
    Ok(())
}

#[test]
fn home_camera_localization_accepts_a_different_known_portal_without_seeing_ucof() -> Result<()> {
    use auki_components::{ComponentRuntime, Observation, OutputReference};
    use auki_odometry::{PoseContract, PoseUpdate, Tracking};
    use base64::Engine;
    use spatial_home::local_mapper::{CameraSample, LocalMap, SensorEvent};
    let frame = "left_arm_camera_color_optical_frame";
    let canonical = canonical()?;
    let owner = "robot-b";
    let runtime = ComponentRuntime::new("home");
    let session = uuid::Uuid::new_v4().to_string();
    let contract = PoseContract {
        from_frame: FrameRegistryEntry::ros_body(owner, "robot.base"),
        to_frame: FrameRegistryEntry::ros_body(owner, "odometry"),
        clock: clock(owner),
        session_id: "session".into(),
    };
    let source = ProductReference {
        peer_id: owner.into(),
        product_id: "local-camera-observations".into(),
        manifest_hash: "camera-hash".into(),
    };
    let mut map = LocalMap::from_runner_with_camera(
        &runtime,
        "home",
        &session,
        contract.clone(),
        source.clone(),
        ProductReference {
            product_id: "lidar".into(),
            ..source.clone()
        },
        None,
        FrameRegistryEntry::ros_optical(owner, frame),
    )?;
    map.calibrate_against(&canonical)?;
    map.emit_localized_scans();
    map.accept_pose(
        100,
        &PoseUpdate {
            contract,
            tracking: Tracking::Valid {
                pose: Pose {
                    translation: [9., -2., 0.5],
                    rotation_xyzw: [0., 0., 0.6, 0.8],
                },
            },
        },
    )?;
    let code = "HRIPLL40GL6";
    let qr = qrcode::QrCode::with_version(
        format!("https://r8.hr/{code}"),
        qrcode::Version::Normal(3),
        qrcode::EcLevel::M,
    )?;
    let mut image = vec![255; 1280 * 720];
    let modules = qr.width();
    for y in 0..modules {
        for x in 0..modules {
            if qr[(x, y)] == qrcode::Color::Dark {
                for dy in 0..8 {
                    for dx in 0..8 {
                        image[(360 - modules * 4 + y * 8 + dy) * 1280 + 640 - modules * 4
                            + x * 8
                            + dx] = 0;
                    }
                }
            }
        }
    }
    let camera = CameraSample {
        timestamp_ns: 100,
        rgb_timestamp_ns: 100,
        transform_timestamp_ns: 100,
        width: 1280,
        height: 720,
        fx: 500.,
        fy: 500.,
        cx: 640.,
        cy: 360.,
        gray_base64: base64::engine::general_purpose::STANDARD.encode(image),
        calibration: spatial_home::runner_voxels::SensorCalibration {
            sensor_frame: frame.into(),
            sensor_to_base: Pose {
                translation: [0.3, -0.4, 0.8],
                rotation_xyzw: [0.5, 0., 0., 3_f64.sqrt() / 2.],
            },
            calibration_source: format!(
                "Galbot SDK D405 co-registered RGB-D; RealSense deprojection; sha256:{}",
                "a".repeat(64)
            ),
        },
        hit_points_m: vec![[0., 0., 0.25]],
        hit_colors_srgb: vec![[255, 0, 0]],
    };
    let sensor_to_base = camera.calibration.sensor_to_base.clone();
    let image_b64 = camera.gray_base64.clone();
    map.accept_remote(Observation {
        output: OutputReference {
            peer_id: owner.into(),
            component_id: "source".into(),
            component_manifest_hash: "component".into(),
            slot: "camera".into(),
            output_id: "camera".into(),
            manifest_hash: "camera-hash".into(),
        },
        sequence: 42,
        timestamp_ns: 100,
        payload: std::sync::Arc::new(SensorEvent::Camera { sample: camera }),
    })?;
    let expected = reference_in_frame(&canonical, &map.frame)?
        .scenegraph
        .anchors[code]
        .pose_in_map
        .clone();
    let actual = map.portals()?.scenegraph.anchors[code].pose_in_map.clone();
    assert!(
        actual
            .translation
            .iter()
            .zip(expected.translation)
            .all(|(a, b)| (a - b).abs() < 1e-8)
    );
    assert_eq!(
        map.origin
            .as_ref()
            .unwrap()
            .calibration
            .as_ref()
            .unwrap()
            .portal_id,
        code
    );
    assert!(
        !map.portals()?
            .scenegraph
            .anchors
            .contains_key("UCOF1J40N7O")
    );
    let scans = map.take_localized_scans();
    assert_eq!(scans.len(), 1);
    assert_eq!(scans[0].observation.source, source);
    assert_eq!(scans[0].observation.sequence, 42);
    let mut fusion = CollaborativeMap::new("home", &uuid::Uuid::new_v4().to_string(), &canonical)?;
    fusion.peer_progress(owner, map.metadata(), &map.portals()?)?;
    assert!(fusion.integrate(scans[0].clone())?);
    let scene = fusion.scene()?.unwrap();
    assert_eq!(scene.voxels.integrated_observations, 1);
    let landmarks = spatial_home::landmarks::from_registries(&scene.registries, &scene.frame)?;
    let local_landmarks = map.landmarks(&map.portals()?)?;
    let corner_id = format!("{code}/top_left");
    assert_eq!(
        landmarks.landmarks[&corner_id].provenance,
        local_landmarks.landmarks[&corner_id].provenance
    );
    assert!(
        landmarks
            .components
            .contains(&local_landmarks.components[0])
    );
    let rendered = spatial_home::scene_mirror::render_scene(&scene)?;
    let display_landmarks: spatial_home::landmarks::Landmarks =
        serde_json::from_value(rendered["landmarks"].clone())?;
    let expected_point = spatial_home::conventions::point(
        landmarks.position_in_map(&corner_id)?,
        &scene.voxels.definition.frame,
        &spatial_home::conventions::registry("home", &display_landmarks.frame)?,
    )?;
    for (a, b) in display_landmarks
        .position_in_map(&corner_id)?
        .into_iter()
        .zip(expected_point)
    {
        assert!((a - b).abs() < 1e-9);
    }
    assert_eq!(
        scene.registries["collaborative_mapping"]["runners"][owner]["origin"]["calibration"]["portal_id"],
        code
    );
    // AR image and optical pose come from the same accepted camera observation.
    let mut ar = map.ar_frame.take().unwrap();
    let preview = spatial_home::ar_preview::render(&ar, &scene)?;
    assert_eq!(preview["data"], image_b64);
    assert_eq!(preview["capture_timestamp_ns"], 100);
    assert_eq!(preview["sequence"], 42);
    assert_eq!(preview["ar"]["intrinsics"]["fx"], 500.);
    let robot = Pose {
        translation: [9., -2., 0.5],
        rotation_xyzw: [0., 0., 0.6, 0.8],
    };
    let expected = spatial_home::local_mapper::compose(
        &map.origin.as_ref().unwrap().odometry_to_local,
        &spatial_home::local_mapper::compose(&robot, &sensor_to_base)?,
    )?;
    assert_eq!(ar.camera_to_map, expected);
    let display = spatial_home::local_mapper::display_frame(&scene.frame)?;
    let expected = spatial_home::conventions::pose(
        &expected,
        &scene.voxels.definition.frame,
        &spatial_home::conventions::registry("home", &display)?,
    )?;
    assert_eq!(
        preview["ar"]["camera_to_map"],
        serde_json::to_value(expected)?
    );
    let original = ar.clone();
    ar.clock.peer_id = "different-robot".into();
    assert!(spatial_home::ar_preview::render(&ar, &scene).is_err());
    ar = original.clone();
    ar.origin.odometry_to_local.translation[0] += 1.;
    assert!(spatial_home::ar_preview::render(&ar, &scene).is_err());
    ar = original.clone();
    ar.received = std::time::Instant::now() - std::time::Duration::from_secs(6);
    assert!(spatial_home::ar_preview::render(&ar, &scene).is_err());
    // The generic viewer independently joins a camera, a thin SDK pose, and map.
    // Neither the pose nor its Product needs an image or camera calibration.
    let pose_history = map.ar_pose_history();
    let pose_contract = PoseContract {
        from_frame: FrameRegistryEntry::ros_optical(owner, frame),
        to_frame: pose_history.contract.from_frame.clone(),
        clock: clock(owner),
        session_id: "session".into(),
    };
    let mut image_output = auki_components::OutputManifest {
        schema: "auki.component-output-manifest/v3".into(),
        peer_id: owner.into(),
        component_id: "camera".into(),
        component_manifest_hash: "component".into(),
        slot: "samples".into(),
        output_id: "image".into(),
        clock: clock(owner),
        spatial_frame_id: Some(frame.into()),
        spatial_frame: Some(auki_components::RegisteredFrame::new(
            pose_contract.from_frame.clone(),
        )),
        payload: auki_components::PayloadContract::Camera(auki_components::CameraPayloadContract {
            datatype: "video_frame".into(),
            schema: "auki.video-frame/v1".into(),
            encoding: "jpeg".into(),
            width: 1280,
            height: 720,
            nominal_frame_rate_hz: None,
            observes: frame.into(),
        }),
    };
    let mut pose_output = image_output.clone();
    pose_output.output_id = "pose".into();
    pose_output.spatial_frame_id = Some(pose_contract.to_frame.frame_id.clone());
    pose_output.spatial_frame = Some(auki_components::RegisteredFrame::new(
        pose_contract.to_frame.clone(),
    ));
    pose_output.payload =
        auki_components::PayloadContract::Structured(auki_components::StructuredPayloadContract {
            datatype: "auki.odometry-pose".into(),
            schema: "auki.odometry-pose/v1".into(),
            modality: "pose".into(),
            observes: frame.into(),
            unit: None,
        });
    let image = Observation {
        output: image_output.reference(),
        sequence: 1,
        timestamp_ns: 100,
        payload: std::sync::Arc::new(auki_components::VideoFrame {
            width: 1280,
            height: 720,
            encoding: "jpeg".into(),
            bytes: vec![1, 2, 3].into(),
        }),
    };
    let mut sensor_pose = Observation {
        output: pose_output.reference(),
        sequence: 7,
        timestamp_ns: 100,
        payload: std::sync::Arc::new(PoseUpdate {
            contract: pose_contract,
            tracking: Tracking::Valid {
                pose: sensor_to_base,
            },
        }),
    };
    let camera_product = ProductReference {
        peer_id: owner.into(),
        product_id: "camera.any-history".into(),
        manifest_hash: "camera".into(),
    };
    let calibration = serde_json::json!({"K":[500.,0.,640.,0.,500.,360.,0.,0.,1.],"D":[],"distortion_model":"none","width":1280,"height":720});
    let join_cache = spatial_home::ar_preview::Frames::default();
    join_cache
        .lock()
        .unwrap()
        .poses
        .insert(owner.into(), pose_history);
    let joined = spatial_home::ar_preview::camera_pose_preview(
        &camera_product,
        &image,
        &image_output,
        &sensor_pose,
        &pose_output,
        &calibration,
        &join_cache,
        &scene,
    )?;
    assert_eq!(joined["ar"]["status"], "ready");
    assert_eq!(joined["data"], "AQID");
    // Body occupancy joins the same capture-time odometry without embedding a
    // robot pose. Transform the entire grid so its cubes retain their axes.
    let mut volume = spatial_home::robot_occupancy::Sample {
        reference: ProductReference {
            peer_id: owner.into(),
            product_id: "robot.occupancy-history".into(),
            manifest_hash: "body".into(),
        },
        sequence: 1,
        timestamp_ns: 100,
        clock: clock(owner),
        received: std::time::Instant::now(),
        body: spatial_home::robot_occupancy::RobotOccupancy {
            session_id: "session".into(),
            model_sha256: "a".repeat(64),
            voxel_size_m: 0.05,
            grid_origin_m: [0.; 3],
            cells: vec![[1, 2, 3]],
            covered_links: vec!["body".into()],
            missing_links: vec![],
            approximate_links: vec![],
            compute_ms: 10.,
        },
    };
    volume.body.validate()?;
    let rendered = spatial_home::robot_occupancy::render(&volume, &join_cache, &scene, true)?;
    let expected_body = spatial_home::conventions::pose(
        &compose(&map.origin.as_ref().unwrap().odometry_to_local, &robot)?,
        &scene.voxels.definition.frame,
        &spatial_home::conventions::registry("home", &display)?,
    )?;
    assert_eq!(rendered["pose"], serde_json::to_value(expected_body)?);
    assert_eq!(rendered["cells"], serde_json::json!([[1, 2, 3]]));
    let mut ahead = volume.clone();
    ahead.timestamp_ns = 101;
    ahead.sequence = 2;
    let history = std::collections::VecDeque::from([volume.clone(), ahead]);
    let synchronized =
        spatial_home::robot_occupancy::render_latest(&history, &join_cache, &scene, true)?;
    assert_eq!(synchronized["timestamp_ns"], 100);
    volume.body.cells = vec![[8, 9, 10]];
    assert_eq!(
        spatial_home::robot_occupancy::render(&volume, &join_cache, &scene, true)?["cells"],
        serde_json::json!([[8, 9, 10]])
    );
    volume.body.session_id = "restarted".into();
    assert!(spatial_home::robot_occupancy::render(&volume, &join_cache, &scene, true).is_err());
    volume.body.session_id = "session".into();
    volume.received -= std::time::Duration::from_secs(4);
    assert!(spatial_home::robot_occupancy::render(&volume, &join_cache, &scene, true).is_err());
    volume.body.cells = vec![[i32::MIN, 0, 0]];
    assert!(volume.body.validate().is_err());
    sensor_pose.timestamp_ns = 99;
    assert!(
        spatial_home::ar_preview::camera_pose_preview(
            &camera_product,
            &image,
            &image_output,
            &sensor_pose,
            &pose_output,
            &calibration,
            &join_cache,
            &scene
        )
        .is_err()
    );
    sensor_pose.timestamp_ns = 100;
    std::sync::Arc::make_mut(&mut sensor_pose.payload)
        .contract
        .to_frame
        .frame_id = "wrong-base".into();
    assert!(
        spatial_home::ar_preview::camera_pose_preview(
            &camera_product,
            &image,
            &image_output,
            &sensor_pose,
            &pose_output,
            &calibration,
            &join_cache,
            &scene
        )
        .is_err()
    );
    image_output.peer_id = "different-robot".into();
    assert!(
        spatial_home::ar_preview::camera_pose_preview(
            &camera_product,
            &image,
            &image_output,
            &sensor_pose,
            &pose_output,
            &calibration,
            &join_cache,
            &scene
        )
        .is_err()
    );
    let cache = spatial_home::ar_preview::Frames::default();
    cache.lock().unwrap().frames.insert(owner.into(), original);
    assert!(
        spatial_home::ar_preview::preview("robot-b/camera.left_arm-history", &cache, Some(&scene))
            .is_err()
    );
    assert!(
        spatial_home::ar_preview::preview("robot-c/local-camera-history", &cache, Some(&scene))
            .is_err()
    );
    Ok(())
}

#[test]
fn sdk_fisheye_overlay_projects_voxels_and_rejects_behind_camera() -> Result<()> {
    let canonical = canonical()?;
    let mut map = CollaborativeMap::new("home", &uuid::Uuid::new_v4().to_string(), &canonical)?;
    map.integrate(scan(
        &canonical,
        "a",
        "UCOF1J40N7O",
        1,
        100,
        0.55,
        [255, 0, 0],
    )?)?;
    let mut scene = map.scene()?.unwrap();
    scene.registries["text_notes"] = serde_json::json!({"schema":"spatial.text-notes/v1","frame":scene.frame,"items":[{"id":"abc","text":"abc","position":[0.55,0.05,0.05],"anchor_portal":"UCOF1J40N7O","height_m":1.}]});
    let calibration = serde_json::json!({"K":[80.,0.,35.,0.,80.,32.,0.,0.,1.],"D":[0.,0.,0.,0.],"distortion_model":"kannala_brandt","width":100,"height":80});
    let optical = Pose {
        translation: [0.55, 0.05, -1.95],
        ..identity()
    };
    let overlay = spatial_home::ar_preview::projected_voxels(&scene, &optical, &calibration)?;
    assert_eq!(overlay["voxel_count"], 1);
    assert_eq!(overlay["notes"][0]["pixel"], serde_json::json!([35., 32.]));
    assert_eq!(overlay["notes"][0]["text"], "abc");
    let vertices = overlay["vertices"].as_array().unwrap();
    assert_eq!(vertices.len(), 36);
    let radius = (2_f64).sqrt() * 0.05 / 1.95;
    let expected = 35. - 80. * radius.atan() / (2_f64).sqrt();
    assert!((vertices[0][0].as_f64().unwrap() - expected).abs() < 1e-4);
    let behind = Pose {
        translation: [0.55, 0.05, 2.05],
        ..identity()
    };
    assert_eq!(
        spatial_home::ar_preview::projected_voxels(&scene, &behind, &calibration)?["voxel_count"],
        0
    );
    assert_eq!(
        spatial_home::ar_preview::projected_voxels(&scene, &behind, &calibration)?["notes"],
        serde_json::json!([])
    );
    let mut wrong = calibration;
    wrong["distortion_model"] = serde_json::json!("unknown");
    assert!(spatial_home::ar_preview::projected_voxels(&scene, &optical, &wrong).is_err());
    Ok(())
}

#[test]
fn wrist_projection_matches_installed_vendor_model() -> anyhow::Result<()> {
    use auki_geometry::pnp::{Camera, Vector3};
    let camera = Camera::new(
        652.873046875,
        652.3247680664062,
        630.3496704101562,
        358.522216796875,
        &[
            -0.05156100541353226,
            0.0568576455116272,
            0.001526004751212895,
            0.0005986306350678205,
            -0.017788469791412354,
        ],
    )
    .unwrap();
    for (p, expected) in [
        ([0., 0., 1.], [630.3496704101562, 358.522216796875]),
        ([0.7, -0.4, 1.], [1080.874267578125, 102.08787536621094]),
        ([-0.8, 0.45, 1.], [115.33527374267578, 648.9982299804688]),
        ([0.2, 0.3, 2.], [695.5817260742188, 456.30145263671875]),
    ] {
        let pixel = spatial_home::ar_preview::project_camera_point(
            &camera,
            "Inverse Brown Conrady",
            Vector3::new(p[0], p[1], p[2]),
        )
        .unwrap();
        assert!((pixel.x - expected[0]).abs() < 0.002);
        assert!((pixel.y - expected[1]).abs() < 0.002);
    }
    Ok(())
}

#[test]
fn ar_alignment_history_is_bounded_and_never_uses_future_or_other_session() -> Result<()> {
    let canonical = canonical()?;
    let mut origin = scan(&canonical, "a", "UCOF1J40N7O", 1, 100, 0.55, [255, 0, 0])?.origin;
    let mut cache = spatial_home::ar_preview::Cache::default();
    for t in 1..=70 {
        origin.capture_timestamp_ns = t;
        cache.record_alignment("a", &origin);
    }
    assert_eq!(cache.alignments["a"].len(), 64);
    assert_eq!(
        cache.alignment_at("a", 50, &origin)?.capture_timestamp_ns,
        50
    );
    assert!(cache.alignment_at("a", 5, &origin).is_err());
    origin.capture_clock = clock("different-session");
    origin.capture_timestamp_ns = 100;
    cache.record_alignment("a", &origin);
    assert_eq!(cache.alignments["a"].len(), 1);
    assert!(cache.alignment_at("a", 90, &origin).is_err());
    Ok(())
}

#[test]
fn camera_and_scene_picks_agree_and_surface_notes_persist() -> Result<()> {
    use serde_json::json;
    use spatial_home::picking::{PickRequest, Picks};
    let canonical = canonical()?;
    let mut map = CollaborativeMap::new("home", &uuid::Uuid::new_v4().to_string(), &canonical)?;
    map.integrate(scan(
        &canonical,
        "a",
        "UCOF1J40N7O",
        1,
        100,
        0.55,
        [255, 0, 0],
    )?)?;
    let scene = map.scene()?.unwrap();
    let mut picks = Picks::default();
    let token=picks.remember_camera(json!({"frame":scene.frame,"camera_to_map":{"translation":[0.55,0.05,-1.95],"rotation_xyzw":[0.,0.,0.,1.]},"calibration":{"K":[80.,0.,35.,0.,80.,32.,0.,0.,1.],"D":[0.,0.,0.,0.],"distortion_model":"kannala_brandt","width":100,"height":80}}))?;
    let camera = picks.pick(
        PickRequest::Camera {
            camera_token: token.clone(),
            pixel: [35., 32.],
        },
        &scene,
    )?;
    let target = spatial_home::conventions::registry(
        "home",
        &spatial_home::local_mapper::display_frame(&scene.frame)?,
    )?;
    let origin = spatial_home::conventions::point(
        [0.55, 0.05, -1.95],
        &scene.voxels.definition.frame,
        &target,
    )?;
    let direction =
        spatial_home::conventions::point([0., 0., 1.], &scene.voxels.definition.frame, &target)?;
    let viewer = picks.pick(
        PickRequest::Scene {
            frame: target.frame_id,
            origin,
            direction,
        },
        &scene,
    )?;
    assert_eq!(camera["hit"]["position"], viewer["hit"]["position"]);
    assert!(!camera["hit"].is_null());
    assert!(
        picks
            .pick(
                PickRequest::Camera {
                    camera_token: token.clone(),
                    pixel: [-1., 32.]
                },
                &scene
            )
            .is_err()
    );
    let mut newer = scene.clone();
    newer.frame = "home-live:other:zup".into();
    assert!(
        picks
            .pick(
                PickRequest::Camera {
                    camera_token: token,
                    pixel: [35., 32.]
                },
                &newer
            )
            .is_err()
    );
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("home.json");
    let mut store = spatial_home::model::Store::open_canonical(&path)?;
    let hit_token = camera["hit"]["token"].as_str().unwrap();
    let note =
        spatial_home::notes::add_surface(&mut store, "surface".into(), picks.contact(hit_token)?)?;
    picks.consume(hit_token);
    assert!(picks.contact(hit_token).is_err());
    assert_eq!(json!(note.position), camera["hit"]["position"]);
    let mut loaded = spatial_home::model::Store::open_canonical(&path)?;
    spatial_home::notes::initialize(&mut loaded)?;
    assert_eq!(
        spatial_home::notes::from_registries(&loaded.built.registries, spatial_home::model::FRAME)?
            .items[0],
        note
    );
    Ok(())
}

mod support;
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_subscribes_to_live_geometry_without_replacing_canonical_portals() -> Result<()> {
    use auki_component_protocol::RemoteObservationEvent;
    use spatial_home::{
        mobile_geometry,
        network::{HomeConnection, ScenePublisher, subscribe_scene_product},
    };
    let pair = support::LocalPair::start().await?;
    let result = async {
        let canonical = canonical()?;
        let owner = pair.a.peer_id().to_string();
        let mut map = CollaborativeMap::new(&owner, &uuid::Uuid::new_v4().to_string(), &canonical)?;
        map.integrate(scan(
            &canonical,
            "a",
            "UCOF1J40N7O",
            1,
            100,
            0.55,
            [255, 0, 0],
        )?)?;
        let scene = map.scene()?.unwrap();
        let mut publisher = ScenePublisher::mount(&pair.a)?;
        publisher.enable_home_live()?;
        publisher.publish_home_live(&scene)?;
        let home = HomeConnection::direct(&pair.a);
        let mut subscription = subscribe_scene_product(&pair.b, &home, "home-live-scene").await?;
        let event =
            tokio::time::timeout(std::time::Duration::from_secs(5), subscription.next()).await??;
        let Some(RemoteObservationEvent::Observation(o)) = event else {
            anyhow::bail!("missing live scene")
        };
        o.payload.validate(&owner)?;
        let render = mobile_geometry::render(&o.payload)?;
        assert!(!render["voxels"].as_array().unwrap().is_empty());
        assert_eq!(render["frame"], spatial_home::model::FRAME);
        let mut reference = serde_json::to_value(&canonical)?;
        reference["render"] =
            serde_json::json!({"notes":[],"voxels":[],"frame":spatial_home::model::FRAME});
        let merged = mobile_geometry::merge(&reference, Some(&render));
        assert_eq!(merged["scenegraph"], reference["scenegraph"]);
        assert_eq!(merged["render"]["voxels"], render["voxels"]);
        let dir = tempfile::tempdir()?;
        let mut store =
            spatial_home::model::Store::open_canonical(dir.path().join("phone-notes.json"))?;
        spatial_home::notes::initialize(&mut store)?;
        let center: [f64; 3] = serde_json::from_value(render["voxels"][0]["center"].clone())?;
        let note = spatial_home::mobile_notes::place_with_live(
            &mut store,
            spatial_home::mobile_notes::PlaceNote {
                id: uuid::Uuid::new_v4().to_string(),
                frame: spatial_home::model::FRAME.into(),
                origin: [center[0], center[1] + 1., center[2]],
                direction: [0., -1., 0.],
                text: "live voxel".into(),
            },
            Some(&scene),
        )?;
        assert!(
            !note.anchor_surface.as_ref().unwrap().floor,
            "live voxel intercepts the floor"
        );
        assert!(
            (note.position[1] - (center[1] + render["voxel_size"].as_f64().unwrap() / 2.)).abs()
                < 1e-6
        );
        let mut unaligned = scene.clone();
        unaligned.registries["local_mapping"]["origin"]["kind"] = serde_json::json!("robot_boot");
        assert!(mobile_geometry::render(&unaligned).is_err());
        subscription.close().await?;
        publisher.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}
