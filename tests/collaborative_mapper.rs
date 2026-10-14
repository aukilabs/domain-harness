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
fn dense_capture_retains_every_colored_surface_hit_under_ray_budget() -> Result<()> {
    let canonical = canonical()?;
    let mut map = CollaborativeMap::new("home", &uuid::Uuid::new_v4().to_string(), &canonical)?;
    let mut input = scan(&canonical, "a", "UCOF1J40N7O", 1, 100, 4., [255, 0, 0])?;
    // A dense measured plane: over both the SDK point limit and ray-step budget.
    let size = spatial_home::collaborative_mapper::HOME_VOXEL_SIZE;
    input.observation.hit_points_m = (0..2048)
        .map(|i| [3., (i % 32) as f64 * size, (i / 32) as f64 * size]).collect();
    input.colors = (0..2048).map(|i| Some(if i % 2 == 0 { [255, 0, 0] } else { [0, 0, 255] })).collect();
    assert!(map.integrate(input.clone())?);
    let update = auki_datatypes::map::MapUpdate::decode(map.voxels.as_ref().unwrap().checkpoint.as_slice())?;
    let occupied: Vec<_> = update.checkpoint.unwrap().voxel_chunks.into_iter()
        .flat_map(|c| c.voxels).filter(|v| v.occupancy_evidence > 0.).collect();
    assert_eq!(occupied.len(), 2048, "Ray thinning must not discard any surface cell");
    assert!(occupied.iter().all(|v| v.color.as_ref().is_some_and(|c| c.weight == 1.)));
    assert_eq!(occupied.iter().filter(|v| v.color.as_ref().unwrap().red_sum > 0.9).count(), 1024);
    assert_eq!(occupied.iter().filter(|v| v.color.as_ref().unwrap().blue_sum > 0.9).count(), 1024);
    assert!(!map.integrate(input.clone())?, "A retained capture cannot be replayed");
    // Invalid endpoints must be rejected even when ray subsampling would omit them.
    input.observation.sequence = 2;
    input.observation.timestamp_ns = 200;
    input.observation.pose.timestamp_ns = 200;
    input.observation.hit_points_m[1][0] = f64::NAN;
    assert!(map.integrate(input).is_err());
    assert_eq!(map.voxels.as_ref().unwrap().integrated_observations, 1);
    Ok(())
}

#[test]
fn explicit_clear_preserves_reference_and_rejects_old_observations() -> Result<()> {
    let canonical=canonical()?;
    let mut map=CollaborativeMap::new("home", &uuid::Uuid::new_v4().to_string(), &canonical)?;
    let input=scan(&canonical,"a","UCOF1J40N7O",1,100,0.55,[255,0,0])?;
    assert!(map.integrate(input.clone())?);
    assert!(!occupied_x(&map).is_empty());
    let portals=serde_json::to_value(map.portals()?)?;
    map.clear_voxels()?;
    assert!(occupied_x(&map).is_empty());
    let scene=map.scene()?.unwrap();
    assert!(auki_datatypes::map::MapUpdate::decode(scene.voxels.checkpoint.as_slice())?.checkpoint.unwrap().voxel_chunks.is_empty());
    assert_eq!(serde_json::to_value(map.portals()?)?,portals);
    assert!(!map.integrate(input)?);
    assert!(map.integrate(scan(&canonical,"a","UCOF1J40N7O",2,200,0.55,[255,0,0])?)?);
    assert!(!occupied_x(&map).is_empty());
    Ok(())
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
    assert_eq!(map.voxels.as_ref().unwrap().definition.grid().voxel_size_m.0,0.1);
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
    // The first corner of the 10cm cell containing [0.55,0.05,0.05]
    // is [0.5,0.,0.].
    let radial=0.05_f64.hypot(0.05);
    let expected = 35. - 80. * (radial/1.95).atan() * 0.05/radial;
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
