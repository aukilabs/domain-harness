mod support;
use anyhow::Result;
use spatial_home::{
    local_mapper::identity,
    scene_contribution::{Contribution, Inbox},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
fn rigid(p: &auki_odometry::Pose, from: &str, to: &str) -> auki_scenegraph::RigidTransform {
    let [x, y, z, w] = p.rotation_xyzw;
    auki_scenegraph::RigidTransform {
        from_frame_id: from.into(),
        to_frame_id: to.into(),
        translation: p.translation,
        rotation_wxyz: [w, x, y, z],
    }
}

fn sample(peer: &str, reference: &auki_scenegraph::MapSnapshot) -> Contribution {
    let frame = &reference.scenegraph.map.frame.id;
    let anchor = reference.scenegraph.anchors.values().next().unwrap();
    let mut portal = anchor.pose_in_map.clone();
    portal.to_frame_id = "native".into();
    Contribution {
        source: auki_components::ProductReference {
            peer_id: peer.into(),
            product_id: "measured-depth".into(),
            manifest_hash: "manifest".into(),
        },
        sequence: 1,
        clock: spatial_home::model::clock_entry(peer, "session"),
        timestamp_ns: 200,
        sensor_frame: auki_registry::FrameRegistryEntry::ros_optical(peer, "sensor"),
        native_frame: auki_registry::FrameRegistryEntry::opengl(peer, "native"),
        sensor_to_native: rigid(&identity(), "sensor", "native"),
        alignment_timestamp_ns: 100,
        portal_id: anchor.anchor_id.clone(),
        portal_to_native: portal,
        native_to_scene: rigid(&identity(), "native", frame),
        points: vec![[0., 0., 1.]],
        colors: vec![Some([40, 80, 120])],
    }
}

#[test]
fn contribution_localization_preserves_all_surface_points_and_colors() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scenes = support::seeded_local_scenes(dir.path())?;
    let reference = scenes.snapshot("humanoid-hub-conference")?;
    let mut contribution = sample("bracketbot", reference);
    contribution.points = (0..2048).map(|i| [i as f64 / 1000., 0., 4.]).collect();
    contribution.colors = (0..2048).map(|i| Some([i as u8, 80, 120])).collect();
    let snapshot = auki_scenegraph::component::SnapshotReference {
        product: contribution.source.clone(), sequence: 1,
    };
    let localized = contribution.clone().localize(reference, snapshot)?;
    assert_eq!(localized.observation.hit_points_m, contribution.points);
    assert_eq!(localized.colors, contribution.colors);
    Ok(())
}

#[test]
fn home_retains_latest_native_relationship_without_replaying_samples() -> Result<()> {
    let dir=tempfile::tempdir()?;let scenes=support::seeded_local_scenes(dir.path())?;
    let reference=scenes.snapshot("humanoid-hub-conference")?;
    let mut map=spatial_home::collaborative_mapper::CollaborativeMap::for_scene("home","session",reference)?;
    let first=sample("bracketbot",reference);
    assert!(map.integrate_contribution(first.clone())?);
    let mut newer=first.clone();newer.sequence=2;newer.timestamp_ns=400;newer.alignment_timestamp_ns=300;
    newer.native_to_scene.translation=[2.,0.,0.];
    newer.portal_to_native.translation[0]-=2.;
    assert!(map.integrate_contribution(newer)?);
    let mut delayed=first;delayed.sequence=3;delayed.timestamp_ns=500;
    assert!(map.integrate_contribution(delayed)?);
    let scene=map.scene()?.unwrap();
    assert_eq!(scene.voxels.integrated_observations,3);
    assert_eq!(scene.registries["input_registries"]["peer:bracketbot"]["native_to_scene"]["translation"],serde_json::json!([2.,0.,0.]));
    Ok(())
}

#[test]
fn different_devices_share_one_scene_without_changing_native_data() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scenes = support::seeded_local_scenes(dir.path())?;
    let reference = scenes.snapshot("humanoid-hub-conference")?;
    let mut map = spatial_home::collaborative_mapper::CollaborativeMap::for_scene(
        "home", "session", reference,
    )?;
    let mut inbox = Inbox::default();
    for peer in ["android", "iphone", "bracketbot"] {
        let c = sample(peer, reference);
        let original = serde_json::to_value(&c)?;
        assert!(inbox.accept(peer, c.clone(), reference)?);
        assert!(!inbox.accept(peer, c.clone(), reference)?);
        assert!(map.integrate_contribution(inbox.take().pop().unwrap())?);
        assert_eq!(serde_json::to_value(&c)?, original);
    }
    let scene = map.scene()?.unwrap();
    assert_eq!(scene.frame, reference.scenegraph.map.frame.id);
    assert_eq!(scene.voxels.integrated_observations, 3);
    assert_eq!(scene.voxels.definition.sources.len(), 3);
    Ok(())
}

#[test]
fn measured_portal_frame_alias_is_explicitly_bound_to_reference() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scenes = support::seeded_local_scenes(dir.path())?;
    let reference = scenes.snapshot("humanoid-hub-conference")?;
    let anchor = &reference.scenegraph.anchors["HNY3JGNLRAE"];
    let mut measured = anchor.clone();
    measured.pose_in_map.from_frame_id = "portal-HNY3JGNLRAE".into();
    measured.pose_in_map.to_frame_id = "native".into();
    let mut c = sample("bracketbot", reference);
    c.portal_to_native = measured.pose_in_map.clone();
    assert!(c.validate("bracketbot", reference).is_err());
    c.portal_to_native = spatial_home::scene_contribution::portal_measurement(&measured, anchor)?;
    c.validate("bracketbot", reference)?;
    assert_eq!(c.portal_to_native.translation, measured.pose_in_map.translation);
    assert_eq!(c.portal_to_native.rotation_wxyz, measured.pose_in_map.rotation_wxyz);
    measured.side_length_m *= 2.;
    assert!(spatial_home::scene_contribution::portal_measurement(&measured, anchor).is_err());
    measured = anchor.clone();
    measured.anchor_id = "different-portal".into();
    assert!(spatial_home::scene_contribution::portal_measurement(&measured, anchor).is_err());
    Ok(())
}

#[test]
fn translated_native_alignment_is_accepted_without_mutating_source() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scenes = support::seeded_local_scenes(dir.path())?;
    let reference = scenes.snapshot("humanoid-hub-conference")?;
    let mut c = sample("bracketbot", reference);
    c.native_to_scene.translation = [2., 0., 0.];
    c.portal_to_native.translation[0] -= 2.;
    c.sensor_to_native.translation = [0.5, 0., 0.];
    let original = serde_json::to_value(&c)?;
    let mut map = spatial_home::collaborative_mapper::CollaborativeMap::for_scene(
        "home", "session", reference,
    )?;
    assert!(c.validate("bracketbot", reference).is_ok());
    assert!(map.integrate_contribution(c.clone())?);
    let scene = map.scene()?.unwrap();
    assert_eq!(serde_json::to_value(&c)?, original);
    assert_eq!(scene.voxels.integrated_observations, 1);
    assert_eq!(scene.frame, reference.scenegraph.map.frame.id);
    Ok(())
}

#[test]
fn rejects_wrong_owner_scene_clock_transform_and_stale_sequence() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scenes = support::seeded_local_scenes(dir.path())?;
    let reference = scenes.snapshot("humanoid-hub-conference")?;
    let c = sample("robot", reference);
    assert!(c.validate("other", reference).is_err());
    assert!(c.validate("robot", scenes.snapshot("nacs")?).is_err());
    let mut bad = c.clone();
    bad.native_to_scene.translation[0] = 1.;
    assert!(bad.validate("robot", reference).is_err());
    let mut bad = c.clone();
    bad.clock.peer_id = "other".into();
    assert!(bad.validate("robot", reference).is_err());
    let mut bad = c.clone();
    bad.timestamp_ns = 99;
    assert!(bad.validate("robot", reference).is_err());
    let mut bad = c.clone();
    bad.sensor_to_native.from_frame_id = "wrong".into();
    assert!(bad.validate("robot", reference).is_err());
    let mut bad = c.clone();
    bad.points[0][0] = f64::NAN;
    assert!(bad.validate("robot", reference).is_err());
    let mut inbox = Inbox::default();
    assert!(inbox.accept("robot", c.clone(), reference)?);
    let mut older = c;
    older.sequence = 2;
    older.timestamp_ns -= 1;
    assert!(!inbox.accept("robot", older, reference)?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generic_receiver_is_scene_scoped_and_phone_api_remains_advertised() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let scenes = Arc::new(support::seeded_local_scenes(dir.path())?);
        let server = spatial_home::network::ScenePublisher::mount(&pair.a)?;
        let clock = spatial_home::model::clock_entry(&pair.a.peer_id().to_string(), "home-session");
        let mut publishers = spatial_home::local_mobile::Publisher::mount_all(
            &server.runtime,
            &server.endpoint,
            scenes.clone(),
            clock,
            Instant::now(),
        )?;
        let home = scenes.connection(
            "humanoid-hub-conference",
            spatial_home::network::HomeConnection::direct(&pair.a),
        )?;
        let c = sample(
            &pair.b.peer_id().to_string(),
            scenes.snapshot("humanoid-hub-conference")?,
        );
        assert!(spatial_home::scene_contribution::send(&pair.b, &home, &c).await?);
        assert!(!spatial_home::scene_contribution::send(&pair.b, &home, &c).await?);
        let other = scenes.connection(
            "nacs",
            spatial_home::network::HomeConnection::direct(&pair.a),
        )?;
        assert!(spatial_home::scene_contribution::send(&pair.b, &other, &c)
            .await
            .is_err());
        tokio::time::sleep(Duration::from_millis(1050)).await;
        for p in &mut publishers {
            p.tick()?;
        }
        assert_eq!(
            scenes.view("humanoid-hub-conference")?["integrated_observations"],
            1
        );
        assert_eq!(scenes.view("nacs")?["integrated_observations"], 0);
        let catalog = server.runtime.catalog().snapshot();
        for prefix in ["home-phone-depth", "scene-contributions"] {
            assert!(catalog
                .components
                .iter()
                .any(|c| c.manifest.component_id == format!("{prefix}:humanoid-hub-conference")));
        }
        server.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(pair.shutdown().await)
}
