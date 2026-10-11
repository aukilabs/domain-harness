use anyhow::Result;
use auki_components::ComponentRuntime;
use auki_odometry::Pose;
use spatial_home::local_mapper::*;

#[test]
fn scan_diagnostics_distinguish_live_poses_from_actual_integration() -> Result<()> {
    let runtime = ComponentRuntime::new("robot");
    let mut map = LocalMap::new(&runtime, "robot", &uuid::Uuid::new_v4().to_string())?;
    map.accept(SensorEvent::Odometry { timestamp_ns:100_000_000, pose:identity() })?;
    map.accept(SensorEvent::Camera { sample:sample("HRIPLL40GL6",200_000_000) })?;
    map.accept(SensorEvent::Odometry { timestamp_ns:500_000_000, pose:identity() })?;
    assert_eq!(map.scan_progress()["status"],"waiting_for_first_scan");
    assert_eq!(map.scan_progress()["pending"],1);
    map.accept(SensorEvent::Odometry { timestamp_ns:3_000_000_000, pose:identity() })?;
    assert_eq!(map.scan_progress()["diagnostics"]["expired_without_pose"],1);
    map.accept(SensorEvent::Camera { sample:sample("HRIPLL40GL6",3_000_000_000) })?;
    let progress=map.scan_progress();
    assert_eq!(progress["integrated_observations"],1);
    assert_eq!(progress["last_integrated_timestamp_ns"],3_000_000_000u64);
    assert_eq!(progress["diagnostics"]["received"],2);
    assert_eq!(progress["diagnostics"]["pose_matched"],1);
    assert_eq!(progress["status"],"integrating");
    map.accept(SensorEvent::TrackingLost { timestamp_ns:3_100_000_000 })?;
    assert_eq!(map.scan_progress()["status"],"tracking_unavailable");
    Ok(())
}

#[test]
fn correction_invalidates_geometry_without_recreating_components_or_changing_session() -> Result<()> {
    let runtime = ComponentRuntime::new("robot");
    let mut map = LocalMap::new(&runtime, "robot", &uuid::Uuid::new_v4().to_string())?;
    let t = 100_000_000;
    map.accept(SensorEvent::Odometry { timestamp_ns:t, pose:identity() })?;
    map.accept(SensorEvent::Camera { sample:sample("HRIPLL40GL6",t) })?;
    assert!(!map.portals()?.scenegraph.anchors.is_empty());
    assert!(map.voxels.as_ref().unwrap().integrated_observations > 0);
    map.accept_home_portals(map.portals()?, "home".into())?;
    let frame = map.frame.clone();
    let origin = map.origin.as_ref().unwrap().odometry_to_local.clone();
    let catalog = serde_json::to_value(runtime.catalog().snapshot())?;
    let corrected = Pose { translation:[2.,0.,0.], ..identity() };
    map.accept(SensorEvent::MapCorrection { timestamp_ns:t*2, pgo_count:2, pose:corrected.clone() })?;
    assert_eq!(map.frame,frame);
    assert_eq!(map.origin.as_ref().unwrap().odometry_to_local,origin);
    assert_eq!(map.latest_pose.as_ref().unwrap().1,corrected);
    assert!(map.portals()?.scenegraph.anchors.is_empty());
    assert!(map.home_portals.is_none());
    map.portals()?.validate()?;
    assert_eq!(map.voxels.as_ref().unwrap().integrated_observations,0);
    assert!(map.voxels.as_ref().unwrap().accumulator()?.viewer_snapshot(0.)?.chunks.is_empty());
    assert_eq!(map.metadata()["local_mapping"]["map_correction"]["pgo_count"],2);
    // A late frame from before the correction must never restore old geometry.
    map.accept(SensorEvent::Camera { sample:sample("HRIPLL40GL6",t+1) })?;
    assert!(map.portals()?.scenegraph.anchors.is_empty());
    assert_eq!(map.voxels.as_ref().unwrap().integrated_observations,0);
    map.accept(SensorEvent::Camera { sample:sample("HRIPLL40GL6",t*2) })?;
    assert_eq!(map.portals()?.scenegraph.anchors.len(),1);
    assert_eq!(map.voxels.as_ref().unwrap().integrated_observations,1);
    // Current outputs and products stay the same across repeated corrections.
    for n in 3..6 {
        map.accept(SensorEvent::MapCorrection { timestamp_ns:t*n, pgo_count:n as i64, pose:corrected.clone() })?;
        assert!(map.portals()?.scenegraph.anchors.is_empty());
        map.portals()?.validate()?;
    }
    let after = serde_json::to_value(runtime.catalog().snapshot())?;
    assert_eq!(catalog["components"],after["components"]);
    map.accept(SensorEvent::TrackingLost { timestamp_ns:t*6 })?;
    assert!(map.latest_pose.is_none());
    assert_eq!(map.metadata()["local_mapping"]["status"],"waiting_for_odometry");
    map.accept(SensorEvent::Odometry { timestamp_ns:t*7, pose:corrected })?;
    assert!(map.latest_pose.is_some());
    Ok(())
}

#[test]
fn decoded_portal_with_unknown_size_is_not_reported_as_no_detection() -> Result<()> {
    let runtime = ComponentRuntime::new("robot");
    let mut map = LocalMap::new(&runtime, "robot", &uuid::Uuid::new_v4().to_string())?;
    map.accept(SensorEvent::Odometry { timestamp_ns:100_000_000, pose:identity() })?;
    map.accept(SensorEvent::Camera { sample:sample("ZZZZZZZZZZZ",100_000_000) })?;
    let diagnostic=map.metadata()["local_mapping"]["camera_scans"][spatial_home::rgbd::FRAME].clone();
    assert_eq!(diagnostic["decoded_qrs"],1);
    assert_eq!(diagnostic["mapped_portals"],0);
    assert_eq!(diagnostic["rejections"][0]["reason"],"printed size unknown");
    assert_eq!(map.landmark_product().latest_existing()?.unwrap().payload.landmarks.len(),4);
    assert!(map.portals()?.scenegraph.anchors.is_empty());
    Ok(())
}

#[test]
fn stereo_eyes_keep_separate_calibration_and_provenance() -> Result<()> {
    use spatial_home::rgbd::{BRACKETBOT_CALIBRATION, BRACKETBOT_FRAME, BRACKETBOT_RIGHT_FRAME};
    let runtime = ComponentRuntime::new("robot");
    let mut map = LocalMap::new_for_robot(
        &runtime,
        "robot",
        &uuid::Uuid::new_v4().to_string(),
        BRACKETBOT_FAMILY,
        BRACKETBOT_FRAME,
        serde_json::from_str(include_str!(
            "../config/humanoid-hub-conference/portals.json"
        ))?,
    )?;
    let mut positions = vec![];
    for (i, frame) in [BRACKETBOT_FRAME, BRACKETBOT_RIGHT_FRAME, BRACKETBOT_FRAME]
        .iter()
        .enumerate()
    {
        let t = 100_000_000 + i as u64 * 100_000_000;
        map.accept(SensorEvent::Odometry {
            timestamp_ns: t,
            pose: identity(),
        })?;
        let mut camera = sample("HNY3JGNLRAE", t);
        camera.calibration.sensor_frame = (*frame).into();
        camera.calibration.calibration_source = format!("{BRACKETBOT_CALIBRATION}{frame}");
        camera.calibration.sensor_to_base.translation[0] = if i == 1 { 0.0645 } else { 0. };
        map.accept(SensorEvent::Camera { sample: camera })?;
        let diagnostic = map.metadata()["local_mapping"]["camera_scans"][*frame].clone();
        assert_eq!(diagnostic["decoded_qrs"], 1);
        assert_eq!(diagnostic["mapped_portals"], 1);
        assert_eq!(diagnostic["rejections"], serde_json::json!([]));
        let batch = map.landmark_product().latest_existing()?.unwrap();
        assert_eq!(batch.payload.landmarks.len(), 4);
        assert_eq!(
            batch.payload.source_observation.output.component_id,
            if i == 1 {
                "local-camera-right"
            } else {
                "local-camera"
            }
        );
        positions.push(
            map.portals()?.scenegraph.anchors["HNY3JGNLRAE"]
                .pose_in_map
                .translation[0],
        );
    }
    assert!((positions[1] - positions[0] - 0.0645).abs() < 1e-6);
    assert!((positions[2] - positions[0]).abs() < 1e-6);
    let sources = &map.voxels.as_ref().unwrap().definition.sources;
    assert!(
        sources
            .iter()
            .any(|s| s.sensor_frame.frame_id == BRACKETBOT_RIGHT_FRAME)
    );
    assert!(
        sources
            .iter()
            .all(|s| s.sensor_product.product_id != "local-lidar-history")
    );
    Ok(())
}

#[test]
fn robot_boot_origin_is_fixed_and_maps_before_any_portal() -> Result<()> {
    let runtime = ComponentRuntime::new("robot");
    let mut map = LocalMap::new(&runtime, "robot", &uuid::Uuid::new_v4().to_string())?;
    let boot = Pose {
        translation: [8., -3., 2.],
        rotation_xyzw: [
            0.,
            0.,
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
        ],
    };
    map.accept(SensorEvent::Odometry {
        timestamp_ns: 100,
        pose: boot.clone(),
    })?;
    assert!(map.voxels.is_some());
    assert!(map.portals()?.scenegraph.anchors.is_empty());
    assert!(
        map.latest_pose
            .as_ref()
            .unwrap()
            .1
            .translation
            .iter()
            .all(|v| v.abs() < 1e-10)
    );
    let origin = map.origin.as_ref().unwrap().odometry_to_local.clone();
    map.accept(SensorEvent::Odometry {
        timestamp_ns: 200,
        pose: Pose {
            translation: [8., -2., 2.],
            ..boot
        },
    })?;
    assert_eq!(map.origin.as_ref().unwrap().odometry_to_local, origin);
    assert!((map.latest_pose.as_ref().unwrap().1.translation[0] - 1.).abs() < 1e-10);
    Ok(())
}
#[test]
fn starts_empty_without_home_portals_or_domain_alignment() -> Result<()> {
    let runtime = ComponentRuntime::new("robot");
    let map = LocalMap::new(&runtime, "robot", &uuid::Uuid::new_v4().to_string())?;
    let snapshot = map.portals()?;
    assert!(snapshot.scenegraph.anchors.is_empty());
    assert!(snapshot.scenegraph.map.domain_reference.is_none());
    assert!(map.voxels.is_none());
    assert_eq!(
        map.metadata()["local_mapping"]["alignment_to_home"],
        serde_json::Value::Null
    );
    assert!(spatial_home::conventions::portals(&snapshot, spatial_home::model::FRAME).is_err());
    assert!(portal_code("https://evil.example/HRIPLL40GL6").is_none());
    Ok(())
}
// A bounded, explicitly supplied sensor recording, never a fabricated fixture.
#[test]
#[ignore = "requires SPATIAL_SENSOR_RECORDING from a read-only Galbot capture"]
fn replay_real_camera_lidar_and_odometry() -> Result<()> {
    use std::io::BufRead;
    let runtime = ComponentRuntime::new("robot");
    let mut map = LocalMap::new(&runtime, "robot", &uuid::Uuid::new_v4().to_string())?;
    let file = std::fs::File::open(std::env::var("SPATIAL_SENSOR_RECORDING")?)?;
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        if let Some(raw) = line.strip_prefix("SENSOR ") {
            let event: SensorEvent = serde_json::from_str(raw)?;
            map.accept(event)?;
        }
    }
    println!("{}", map.metadata()["local_mapping"]);
    assert!(map.origin.is_some());
    assert!(map.voxels.as_ref().unwrap().integrated_observations > 0);
    Ok(())
}

#[test]
fn decoded_camera_portals_define_map_and_colored_depth_uses_same_transform() -> Result<()> {
    let runtime = ComponentRuntime::new("robot");
    let mut map = LocalMap::new(&runtime, "robot", &uuid::Uuid::new_v4().to_string())?;
    for (t, code, x) in [
        (1_000_000_000, "HRIPLL40GL6", 0.),
        (2_000_000_000, "7956Q9STPFK", 0.4),
    ] {
        map.accept(SensorEvent::Odometry {
            timestamp_ns: t,
            pose: Pose {
                translation: [x, 0., 0.],
                ..identity()
            },
        })?;
        map.accept(SensorEvent::Camera {
            sample: sample(code, t),
        })?;
    }
    let portals = map.portals()?;
    assert_eq!(portals.scenegraph.anchors.len(), 2);
    let landmarks = map.landmarks(&portals)?;
    assert_eq!(landmarks.landmarks.len(), 8);
    for point in landmarks.landmarks.values() {
        let detector = point.provenance.detected_by.as_ref().unwrap();
        assert_eq!(detector.peer_id, "robot");
        assert_eq!(detector.component_id, "local-mapper");
        assert_eq!(point.provenance.mapped_by.as_ref(), Some(detector));
        let observation = point.provenance.source_observation.as_ref().unwrap();
        assert_eq!(
            observation.output.component_manifest_hash,
            detector.manifest_hash
        );
        assert_eq!(observation.output.slot, "landmark_observations");
        assert_eq!(
            point
                .provenance
                .camera_observation
                .as_ref()
                .unwrap()
                .timestamp_ns,
            observation.timestamp_ns
        );
    }
    let catalog = runtime.catalog().snapshot();
    assert!(serde_json::to_string(&catalog)?.contains("landmark-observation-history"));
    let detection = map.landmark_product().latest_existing()?.unwrap();
    assert_eq!(
        detection.payload.schema,
        spatial_home::landmarks::OBSERVATIONS_SCHEMA
    );
    assert_eq!(detection.payload.landmarks.len(), 4);
    for (point, name) in detection
        .payload
        .landmarks
        .iter()
        .zip(spatial_home::landmarks::CORNERS)
    {
        assert_eq!(
            point.recognition.landmark_type,
            spatial_home::landmarks::PORTAL
        );
        assert_eq!(point.recognition.identity, format!("7956Q9STPFK/{name}"));
        assert!(point.pixel.iter().all(|p| p.is_finite()));
        let stored = &landmarks.landmarks[&format!("7956Q9STPFK/{name}")];
        assert_eq!(stored.recognition, point.recognition);
        assert_eq!(
            stored
                .provenance
                .source_observation
                .as_ref()
                .unwrap()
                .sequence,
            detection.sequence
        );
        assert_eq!(
            stored.provenance.camera_observation.as_ref(),
            Some(&detection.payload.source_observation)
        );
    }
    let mut invalid = landmarks.clone();
    invalid
        .landmarks
        .values_mut()
        .next()
        .unwrap()
        .provenance
        .detected_by
        .as_mut()
        .unwrap()
        .component_id = "different-detector".into();
    assert!(invalid.validate(&invalid.frame).is_err());
    let mut invalid = landmarks.clone();
    invalid
        .landmarks
        .values_mut()
        .next()
        .unwrap()
        .provenance
        .source_observation = None;
    assert!(invalid.validate(&invalid.frame).is_err());
    let origin = &portals.scenegraph.anchors["HRIPLL40GL6"];
    assert!(origin.pose_in_map.translation[2] > 0.1);
    let second = &portals.scenegraph.anchors["7956Q9STPFK"];
    assert!(
        (second.pose_in_map.translation[0] - origin.pose_in_map.translation[0] - 0.4).abs() < 0.002
    );
    assert!(second.pose_in_map.translation[1].abs() < 0.002);
    assert!((second.pose_in_map.translation[2] - origin.pose_in_map.translation[2]).abs() < 0.002);
    let voxels = map.voxels.as_ref().unwrap();
    assert_eq!(voxels.integrated_observations, 2);
    let scene = spatial_home::network::CanonicalScene {
        schema: spatial_home::network::SCENE_SCHEMA.into(),
        sdk_revision: spatial_home::model::SDK_REV.into(),
        authority_peer_id: "robot".into(),

        frame: map.frame.clone(),
        publication_clock: spatial_home::model::clock_entry("robot", "publication"),
        portals,
        voxels: voxels.clone(),
        objects: vec![],
        registries: {
            let mut metadata = map.metadata();
            metadata["landmarks"] = serde_json::to_value(&landmarks)?;
            metadata
        },
    };
    scene.validate("robot")?;
    let view = spatial_home::scene_mirror::render_scene(&scene)?;
    assert!(
        view["voxels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["linear_rgb"] == serde_json::json!([1., 0., 0.]))
    );
    assert_eq!(view["local_map"], true);
    let rendered: spatial_home::landmarks::Landmarks =
        serde_json::from_value(view["landmarks"].clone())?;
    assert_eq!(rendered.landmarks.len(), 8);
    assert_eq!(
        rendered.landmarks["HRIPLL40GL6/top_left"].provenance,
        landmarks.landmarks["HRIPLL40GL6/top_left"].provenance
    );
    assert_ne!(view["frame"], spatial_home::model::FRAME);
    assert_eq!(view["local_mapping"]["robot_pose"]["frame"], view["frame"]);
    let converted: Pose =
        serde_json::from_value(view["local_mapping"]["robot_pose"]["pose"].clone())?;
    let expected = spatial_home::conventions::pose(
        &map.latest_pose.as_ref().unwrap().1,
        &voxels.definition.frame,
        &spatial_home::conventions::registry("home", view["frame"].as_str().unwrap())?,
    )?;
    assert_eq!(converted.translation, expected.translation);

    // A later measurement of the same ID replaces translation AND orientation.
    // It must not add a duplicate or freeze the first pose.
    let before = map.portals()?.scenegraph.anchors["HRIPLL40GL6"].clone();
    let boot_origin = map.origin.as_ref().unwrap().odometry_to_local.clone();
    let angle = std::f64::consts::FRAC_PI_4;
    let base = Pose {
        translation: [0.8, 0.2, 0.],
        rotation_xyzw: [0., 0., (angle / 2.).sin(), (angle / 2.).cos()],
    };
    map.accept(SensorEvent::Odometry {
        timestamp_ns: 3_000_000_000,
        pose: base.clone(),
    })?;
    map.accept(SensorEvent::Camera {
        sample: sample("HRIPLL40GL6", 3_000_000_000),
    })?;
    let updated = map.portals()?;
    assert_eq!(updated.scenegraph.anchors.len(), 2);
    let after = &updated.scenegraph.anchors["HRIPLL40GL6"];
    let new_landmarks = map.landmarks(&updated)?;
    let before_point = &landmarks.landmarks["HRIPLL40GL6/top_left"];
    let after_point = &new_landmarks.landmarks["HRIPLL40GL6/top_left"];
    assert_eq!(before_point.position, after_point.position);
    assert_ne!(
        landmarks.position_in_map(&before_point.id)?,
        new_landmarks.position_in_map(&after_point.id)?
    );
    assert!(
        after_point
            .provenance
            .source_observation
            .as_ref()
            .unwrap()
            .sequence
            > before_point
                .provenance
                .source_observation
                .as_ref()
                .unwrap()
                .sequence
    );
    let [w, x, y, z] = before.pose_in_map.rotation_wxyz;
    let expected = compose(
        &base,
        &Pose {
            translation: before.pose_in_map.translation,
            rotation_xyzw: [x, y, z, w],
        },
    )?;
    for (actual, expected) in after
        .pose_in_map
        .translation
        .iter()
        .zip(expected.translation)
    {
        assert!((actual - expected).abs() < 1e-8);
    }
    let [x, y, z, w] = expected.rotation_xyzw;
    let dot = after
        .pose_in_map
        .rotation_wxyz
        .iter()
        .zip([w, x, y, z])
        .map(|(a, b)| a * b)
        .sum::<f64>()
        .abs();
    assert!((dot - 1.).abs() < 1e-8);
    assert_ne!(
        after.pose_in_map.rotation_wxyz,
        before.pose_in_map.rotation_wxyz
    );
    assert_eq!(map.origin.as_ref().unwrap().odometry_to_local, boot_origin);
    // A moving wrist contributes its full pose before the robot/base transform.
    // These nontrivial values come from a read-only Galbot TF-chain audit.
    let arm = Pose {
        translation: [0.2902529876782686, -0.1584286616089709, 0.6351196862868895],
        rotation_xyzw: [
            0.6654246158965088,
            -0.607215524187967,
            0.2760524382691864,
            -0.33510362437573443,
        ],
    };
    map.accept(SensorEvent::Odometry {
        timestamp_ns: 4_000_000_000,
        pose: base.clone(),
    })?;
    let mut wrist_sample = sample("HRIPLL40GL6", 4_000_000_000);
    wrist_sample.calibration.sensor_to_base = arm.clone();
    map.accept(SensorEvent::Camera {
        sample: wrist_sample,
    })?;
    // 'before' is the original identity-camera/identity-base observation.
    let [bw, bx, by, bz] = before.pose_in_map.rotation_wxyz;
    let camera_portal = Pose {
        translation: before.pose_in_map.translation,
        rotation_xyzw: [bx, by, bz, bw],
    };
    let expected = compose(&base, &compose(&arm, &camera_portal)?)?;
    let actual = map.portals()?.scenegraph.anchors["HRIPLL40GL6"]
        .pose_in_map
        .clone();
    for i in 0..3 {
        assert!((actual.translation[i] - expected.translation[i]).abs() < 1e-8);
    }
    let [x, y, z, w] = expected.rotation_xyzw;
    let dot: f64 = actual
        .rotation_wxyz
        .iter()
        .zip([w, x, y, z])
        .map(|(a, b)| a * b)
        .sum();
    assert!((dot.abs() - 1.).abs() < 1e-8);
    assert_eq!(
        map.metadata()["local_mapping"]["observed_portals"]["HRIPLL40GL6"]["observations"],
        3
    );
    Ok(())
}

#[test]
#[ignore = "requires SPATIAL_CAMERA_PREVIEW JSON from a real camera Product"]
fn inspect_live_camera_decode() -> Result<()> {
    use base64::Engine;
    let f: serde_json::Value =
        serde_json::from_slice(&std::fs::read(std::env::var("SPATIAL_CAMERA_PREVIEW")?)?)?;
    let mut bytes =
        base64::engine::general_purpose::STANDARD.decode(f["data"].as_str().unwrap())?;
    let width = f["width"].as_u64().unwrap() as usize;
    let height = f["height"].as_u64().unwrap() as usize;
    match f["encoding"].as_str() {
        Some("jpeg" | "jpg") => {
            let image = image::load_from_memory(&bytes)?.to_luma8();
            anyhow::ensure!(
                image.width() as usize == width && image.height() as usize == height,
                "image dimensions disagree"
            );
            bytes = image.into_raw();
        }
        Some("luma8") => {}
        _ => anyhow::bail!("unsupported camera encoding"),
    }
    let mut config = auki_qr_detector::QrDetectorConfig::default();
    if std::env::var("SPATIAL_QR_DIAGNOSTIC_RECOVERY").as_deref() == Ok("1") {
        config.scanner.scan.enable_shadow_normalization = true;
        config.scanner.scan.enable_sharpening = true;
        config.scanner.scan.enable_deblur = true;
        config.scanner.scan.max_variants_per_frame = 64;
    }
    let mut detector = auki_qr_detector::QrDetector::new(config);
    let result = detector.detect_luma(&bytes, width, height, width)?;
    println!("Decoded {} QR codes", result.codes.len());
    for code in result.codes {
        println!("{} mirrored={}", code.payload, code.mirrored);
    }
    Ok(())
}

#[test]
#[ignore = "requires SPATIAL_TRANSFORM_AUDIT capture JSON"]
fn audit_real_portal_transform() -> Result<()> {
    use base64::Engine;
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(std::env::var("SPATIAL_TRANSFORM_AUDIT")?)?)?;
    let sample: CameraSample = serde_json::from_value(value["sample"].clone())?;
    let runtime = ComponentRuntime::new("audit");
    let map = LocalMap::new(&runtime, "audit", &uuid::Uuid::new_v4().to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(&sample.gray_base64)?;
    let detection = auki_qr_detector::QrDetector::default().detect_luma(
        &bytes,
        sample.width as usize,
        sample.height as usize,
        sample.width as usize,
    )?;
    let mut portals = map.portals()?.scenegraph;
    let calibration = auki_qr_localizer::Calibration {
        id: "captured-optics".into(),
        camera_product: auki_components::ProductReference {
            peer_id: "audit".into(),
            product_id: "recorded-camera".into(),
            manifest_hash: "offline-capture".into(),
        },
        camera_frame_id: sample.calibration.sensor_frame.clone(),
        clock: spatial_home::model::clock_ref(&spatial_home::model::clock_entry(
            "audit", "offline",
        )),
        width: sample.width,
        height: sample.height,
        fx: sample.fx,
        fy: sample.fy,
        cx: sample.cx,
        cy: sample.cy,
        distortion: auki_qr_localizer::Distortion::None,
    };
    let mut evidence = serde_json::Map::new();
    for d in &detection.codes {
        let Some(code) = portal_code(&d.payload) else {
            continue;
        };

        let anchor = auki_scenegraph::QrAnchor {
            anchor_id: code.into(),
            payload: d.payload.clone(),
            side_length_m: 0.15,
            pose_in_map: auki_scenegraph::RigidTransform::identity(
                format!("portal-{code}"),
                &portals.map.frame.id,
            ),
        };
        anyhow::ensure!(code == "UCOF1J40N7O", "Audit size is for UCOF1J40N7O only");
        let corners = d
            .refined_corners_px
            .as_ref()
            .unwrap_or(&d.corners_px)
            .map(|p| [p.x, p.y]);
        let e = auki_qr_localizer::estimate_camera_pose(
            &portals.map,
            &anchor,
            corners,
            &calibration,
            Default::default(),
        )?;
        let [w, x, y, z] = e.camera_pose_in_map.rotation_wxyz;
        let marker_camera = inverse(&Pose {
            translation: e.camera_pose_in_map.translation,
            rotation_xyzw: [x, y, z, w],
        })?;
        let measured = compose(&sample.calibration.sensor_to_base, &marker_camera)?;
        evidence.insert(code.into(),serde_json::json!({"marker_to_camera":marker_camera,"marker_to_base":measured,"reprojection_rms_px":e.reprojection_rms_px}));
        let [x, y, z, w] = measured.rotation_xyzw;
        portals.anchors.insert(
            code.into(),
            auki_scenegraph::QrAnchor {
                pose_in_map: auki_scenegraph::RigidTransform {
                    translation: measured.translation,
                    rotation_wxyz: [w, x, y, z],
                    ..anchor.pose_in_map.clone()
                },
                ..anchor
            },
        );
    }
    anyhow::ensure!(
        !portals.anchors.is_empty(),
        "No accepted portal in audit capture"
    );
    let report = serde_json::json!({"camera_to_base":sample.calibration.sensor_to_base,"detections":detection,"portals_in_base":portals.anchors,"evidence":evidence});
    std::fs::write(
        std::env::var("SPATIAL_TRANSFORM_REPORT")?,
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", report["evidence"]);
    Ok(())
}

fn sample(code: &str, t: u64) -> CameraSample {
    use base64::Engine;
    let qr = qrcode::QrCode::with_version(
        format!("https://r8.hr/{code}"),
        qrcode::Version::Normal(3),
        qrcode::EcLevel::M,
    )
    .unwrap();
    let modules = qr.width();
    let mut bytes = vec![255; 1280 * 720];
    let offset_x = 640 - modules * 4;
    let offset_y = 360 - modules * 4;
    for y in 0..modules {
        for x in 0..modules {
            if qr[(x, y)] == qrcode::Color::Dark {
                for dy in 0..8 {
                    for dx in 0..8 {
                        bytes[(offset_y + y * 8 + dy) * 1280 + offset_x + x * 8 + dx] = 0;
                    }
                }
            }
        }
    }
    CameraSample {
        timestamp_ns: t,
        rgb_timestamp_ns: t,
        transform_timestamp_ns: t,
        width: 1280,
        height: 720,
        fx: 500.,
        fy: 500.,
        cx: 640.,
        cy: 360.,
        gray_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        calibration: spatial_home::runner_voxels::SensorCalibration {
            sensor_frame: spatial_home::rgbd::FRAME.into(),
            sensor_to_base: identity(),
            calibration_source: format!(
                "Galbot SDK D405 co-registered RGB-D; RealSense deprojection; sha256:{}",
                "a".repeat(64)
            ),
        },
        hit_points_m: vec![[0., 0., 0.25]],
        hit_colors_srgb: vec![[255, 0, 0]],
    }
}

#[test]
fn home_maps_raw_observations_in_ucof_frame_and_keeps_remote_provenance() -> Result<()> {
    use auki_components::*;
    use auki_odometry::{PoseContract, PoseUpdate, Tracking};
    use auki_registry::FrameRegistryEntry;
    use std::sync::Arc;
    let runtime = ComponentRuntime::new("home");
    let robot_session = uuid::Uuid::new_v4().to_string();
    let clock =
        spatial_home::model::clock_ref(&spatial_home::model::clock_entry("robot", &robot_session));
    let contract = PoseContract {
        from_frame: FrameRegistryEntry::ros_body("robot", "base"),
        to_frame: FrameRegistryEntry::ros_body("robot", "odometry"),
        clock: clock.clone(),
        session_id: robot_session,
    };
    let camera_ref = ProductReference {
        peer_id: "robot".into(),
        product_id: "local-camera-observations".into(),
        manifest_hash: "camera-hash".into(),
    };
    let lidar_ref = ProductReference {
        product_id: "local-lidar-history".into(),
        ..camera_ref.clone()
    };
    let mut map = LocalMap::from_runner(
        &runtime,
        "home",
        &uuid::Uuid::new_v4().to_string(),
        contract.clone(),
        camera_ref.clone(),
        lidar_ref,
        Some("UCOF1J40N7O".into()),
    )?;
    let camera_output = OutputReference {
        peer_id: "robot".into(),
        component_id: "source".into(),
        component_manifest_hash: "component-hash".into(),
        slot: "camera".into(),
        output_id: "camera".into(),
        manifest_hash: "camera-hash".into(),
    };
    let base = Pose {
        translation: [8., -3., 2.],
        rotation_xyzw: [0., 0., 0.5, 3_f64.sqrt() / 2.],
    };
    let camera_base = Pose {
        translation: [0.3, -0.4, 0.8],
        rotation_xyzw: [0.5, 0., 0., 3_f64.sqrt() / 2.],
    };
    for (i, code) in ["HRIPLL40GL6", "UCOF1J40N7O"].iter().enumerate() {
        let t = (i as u64 + 1) * 1_000_000_000;
        map.accept_pose(
            t,
            &PoseUpdate {
                contract: contract.clone(),
                tracking: Tracking::Valid { pose: base.clone() },
            },
        )?;
        assert!(map.voxels.is_none(), "Home must not start in robot space");
        let mut camera = sample(code, t);
        camera.calibration.sensor_to_base = camera_base.clone();
        map.accept_remote(Observation {
            output: camera_output.clone(),
            sequence: 40 + i as u64,
            timestamp_ns: t,
            payload: Arc::new(SensorEvent::Camera { sample: camera }),
        })?;
        if i == 0 {
            assert!(map.origin.is_none());
            assert!(map.portals()?.scenegraph.anchors.is_empty());
        }
    }
    let portals = map.portals()?;
    let anchor = &portals.scenegraph.anchors["UCOF1J40N7O"];
    assert!(
        anchor
            .pose_in_map
            .translation
            .iter()
            .all(|x| x.abs() < 1e-8)
    );
    assert!((anchor.pose_in_map.rotation_wxyz[0].abs() - 1.).abs() < 1e-8);
    assert_eq!(map.origin.as_ref().unwrap().kind, "portal:UCOF1J40N7O");
    assert!(map.frame.starts_with("home-live:"));
    let voxels = map.voxels.as_ref().unwrap();
    assert_eq!(voxels.integrated_observations, 1);
    let latest = serde_json::to_value(&voxels.latest_observation)?;
    assert_eq!(latest["source"], serde_json::to_value(camera_ref)?);
    assert_eq!(latest["sequence"], 41);
    assert_eq!(latest["clock"], serde_json::to_value(clock)?);
    assert_eq!(voxels.definition.sources[0].sensor_frame.peer_id, "robot");
    let origin = map.origin.clone().unwrap().odometry_to_local;
    map.accept_pose(
        3_000_000_000,
        &PoseUpdate {
            contract,
            tracking: Tracking::Valid {
                pose: Pose {
                    translation: [9., -3., 2.],
                    ..base
                },
            },
        },
    )?;
    assert_eq!(map.origin.unwrap().odometry_to_local, origin);
    Ok(())
}

#[test]
fn known_portal_reacquisition_corrects_home_even_when_pushed_odometry_is_unchanged() -> Result<()> {
    use auki_components::*;
    use auki_odometry::PoseContract;
    use auki_registry::FrameRegistryEntry;
    let canonical = spatial_home::conventions::portals(
        &spatial_home::domain_portals::load()?,
        spatial_home::model::FRAME,
    )?;
    let runtime = ComponentRuntime::new("home");
    let session = uuid::Uuid::new_v4().to_string();
    let clock = spatial_home::model::clock_ref(&spatial_home::model::clock_entry("echo", &session));
    let camera = ProductReference {
        peer_id: "echo".into(),
        product_id: "camera".into(),
        manifest_hash: "camera".into(),
    };
    let mut home = LocalMap::from_runner(
        &runtime,
        "home",
        &session,
        PoseContract {
            from_frame: FrameRegistryEntry::ros_body("echo", "base"),
            to_frame: FrameRegistryEntry::ros_body("echo", "odom"),
            clock,
            session_id: session.clone(),
        },
        camera.clone(),
        ProductReference {
            product_id: "lidar".into(),
            ..camera
        },
        None,
    )?;
    home.calibrate_against(&canonical)?;
    let frame = home.frame.clone();
    let mut first_pose = None;
    let mut old_portal = None;
    let mut checkpoint = None;
    for (i, code) in ["HRIPLL40GL6", "90221M8VMIC"].iter().enumerate() {
        let t = (i as u64 + 1) * 1_000_000_000;
        // A manual push did not register in the robot's odometry.
        home.accept(SensorEvent::Odometry {
            timestamp_ns: t,
            pose: identity(),
        })?;
        let mut image = sample(code, t);
        if i == 1 {
            image.hit_points_m.clear();
            image.hit_colors_srgb.clear();
        }
        home.accept(SensorEvent::Camera { sample: image })?;
        assert_eq!(
            home.origin
                .as_ref()
                .unwrap()
                .calibration
                .as_ref()
                .unwrap()
                .portal_id,
            *code
        );
        assert_eq!(home.origin.as_ref().unwrap().capture_timestamp_ns, t);
        let expected = spatial_home::collaborative_mapper::reference_in_frame(&canonical, &frame)?;
        let actual = home.portals()?.scenegraph.anchors[*code]
            .pose_in_map
            .clone();
        assert!(
            actual
                .translation
                .iter()
                .zip(expected.scenegraph.anchors[*code].pose_in_map.translation)
                .all(|(a, b)| (a - b).abs() < 1e-8)
        );
        if i == 0 {
            first_pose = Some(home.latest_pose.as_ref().unwrap().1.clone());
            old_portal = Some(actual);
            checkpoint = Some(home.voxels.as_ref().unwrap().checkpoint.clone());
        }
    }
    assert_eq!(home.frame, frame);
    assert_eq!(
        home.portals()?.scenegraph.anchors["HRIPLL40GL6"].pose_in_map,
        old_portal.unwrap()
    );
    assert_eq!(
        home.voxels.as_ref().unwrap().checkpoint,
        checkpoint.unwrap(),
        "correction must not move previous geometry"
    );
    let distance = home
        .latest_pose
        .as_ref()
        .unwrap()
        .1
        .translation
        .iter()
        .zip(first_pose.unwrap().translation)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt();
    assert!(
        distance > 1.,
        "new marker must correct the stuck odometry pose"
    );
    // Another view of the same marker also corrects a later manual displacement.
    home.accept(SensorEvent::Odometry {
        timestamp_ns: 3_000_000_000,
        pose: identity(),
    })?;
    let previous = home.origin.as_ref().unwrap().odometry_to_local.clone();
    let mut moved = sample("90221M8VMIC", 3_000_000_000);
    moved.calibration.sensor_to_base.translation = [0.2, 0., 0.];
    home.accept(SensorEvent::Camera { sample: moved })?;
    assert_ne!(home.origin.as_ref().unwrap().odometry_to_local, previous);
    Ok(())
}

#[test]
fn robot_home_reference_overlay_switches_to_newly_observed_known_portal() -> Result<()> {
    let runtime = ComponentRuntime::new("robot");
    let mut map = LocalMap::new(&runtime, "robot", &uuid::Uuid::new_v4().to_string())?;
    map.accept(SensorEvent::Odometry {
        timestamp_ns: 1_000_000_000,
        pose: identity(),
    })?;
    map.accept(SensorEvent::Camera {
        sample: sample("HRIPLL40GL6", 1_000_000_000),
    })?;
    map.home_portals = Some(spatial_home::home_alignment::HomePortals {
        portals: spatial_home::conventions::portals(
            &spatial_home::domain_portals::load()?,
            spatial_home::model::FRAME,
        )?,
        peer_id: "home".into(),

        anchor: "HRIPLL40GL6".into(),
    });
    let origin = map.origin.as_ref().unwrap().odometry_to_local.clone();
    map.accept(SensorEvent::Odometry {
        timestamp_ns: 2_000_000_000,
        pose: identity(),
    })?;
    map.accept(SensorEvent::Camera {
        sample: sample("90221M8VMIC", 2_000_000_000),
    })?;
    assert_eq!(map.home_portals.as_ref().unwrap().anchor, "90221M8VMIC");
    assert_eq!(
        map.origin.as_ref().unwrap().odometry_to_local,
        origin,
        "robot boot frame remains independent"
    );
    let (aligned, _) = map.home_portals.as_ref().unwrap().align(&map.portals()?)?;
    assert!(
        aligned.scenegraph.anchors["home:90221M8VMIC"]
            .pose_in_map
            .translation
            .iter()
            .zip(
                aligned.scenegraph.anchors["90221M8VMIC"]
                    .pose_in_map
                    .translation
            )
            .all(|(a, b)| (a - b).abs() < 1e-8)
    );
    Ok(())
}

#[test]
fn bracketbot_stereo_maps_room_depth_and_detects_manual_conference_portal() -> Result<()> {
    let runtime = ComponentRuntime::new("bracketbot");
    let session = uuid::Uuid::new_v4().to_string();
    let sizes = serde_json::from_str(include_str!(
        "../config/humanoid-hub-conference/portals.json"
    ))?;
    let mut map = LocalMap::new_for_robot(
        &runtime,
        "bracketbot",
        &session,
        BRACKETBOT_FAMILY,
        spatial_home::rgbd::BRACKETBOT_FRAME,
        sizes,
    )?;
    let t = 1_000_000_000;
    map.accept(SensorEvent::Odometry {
        timestamp_ns: t,
        pose: identity(),
    })?;
    let mut camera = sample("HNY3JGNLRAE", t);
    camera.calibration.sensor_frame = spatial_home::rgbd::BRACKETBOT_FRAME.into();
    camera.calibration.calibration_source = format!(
        "{}{}",
        spatial_home::rgbd::BRACKETBOT_CALIBRATION,
        "a".repeat(64)
    );
    camera.hit_points_m = vec![[0., 0., 2.], [0.1, 0., 3.]];
    camera.hit_colors_srgb = vec![[255, 0, 0], [0, 255, 0]];
    map.accept(SensorEvent::Camera {
        sample: camera.clone(),
    })?;
    assert!(map.frame.starts_with("bracketbot-local:"));
    let portals = map.portals()?;
    let portal = &portals.scenegraph.anchors["HNY3JGNLRAE"];
    assert_eq!(portal.side_length_m, 0.15);
    assert_eq!(map.voxels.as_ref().unwrap().integrated_observations, 1);
    assert_eq!(
        map.ar_pose_history().contract.from_frame,
        auki_registry::FrameRegistryEntry::in_convention(
            "bracketbot",
            "robot.base",
            auki_registry::CoordinateConvention::ZUpRightForward
        )
    );
    map.accept(SensorEvent::TrackingLost {
        timestamp_ns: t + 1,
    })?;
    assert!(map.latest_pose.is_none());
    camera.timestamp_ns += 2;
    camera.rgb_timestamp_ns += 2;
    camera.transform_timestamp_ns += 2;
    map.accept(SensorEvent::Camera { sample: camera })?;
    assert_eq!(map.voxels.as_ref().unwrap().integrated_observations, 1);
    assert!(is_local_frame(&map.frame));
    assert!(!is_local_frame("bracketbot-local:not-a-session:zup"));
    Ok(())
}

#[test]
fn bracketbot_depth_contract_does_not_relax_galbot_validation() -> Result<()> {
    let source = sample("HNY3JGNLRAE", 1_000_000_000);
    let mut capture = spatial_home::rgbd::Capture {
        timestamp_ns: source.timestamp_ns,
        rgb_timestamp_ns: source.rgb_timestamp_ns,
        transform_timestamp_ns: source.transform_timestamp_ns,
        calibration: source.calibration,
        hit_points_m: vec![[0., 0., 2.]],
        hit_colors_srgb: vec![[255, 0, 0]],
    };
    assert!(capture.validate().is_err());
    capture.calibration.sensor_frame = spatial_home::rgbd::BRACKETBOT_FRAME.into();
    assert!(
        capture
            .validate_in_frame(spatial_home::rgbd::BRACKETBOT_FRAME)
            .is_err()
    );
    capture.calibration.calibration_source = format!(
        "{}{}",
        spatial_home::rgbd::BRACKETBOT_CALIBRATION,
        "b".repeat(64)
    );
    capture.validate_in_frame(spatial_home::rgbd::BRACKETBOT_FRAME)?;
    capture.hit_points_m[0] = [4., 0., 4.];
    assert!(
        capture
            .validate_in_frame(spatial_home::rgbd::BRACKETBOT_FRAME)
            .is_err()
    );
    Ok(())
}
