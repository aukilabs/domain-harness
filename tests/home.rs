use serde_json::Value;
use spatial_home::model::*;
fn input() -> (Contribution, Contribution, Detection) {
    let f = fixtures().unwrap();
    (
        serde_json::from_value(f["contributions"][0].clone()).unwrap(),
        serde_json::from_value(f["contributions"][1].clone()).unwrap(),
        serde_json::from_value(f["detection"].clone()).unwrap(),
    )
}
fn view(s: &Store) -> Value {
    serde_json::to_value(&s.built.view).unwrap()
}
#[test]
fn two_sources_align_raycast_and_replay_without_duplicate_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(dir.path().join("home.json")).unwrap();
    let (a, b, d) = input();
    s.apply(Event::Contribution(a.clone())).unwrap();
    s.apply(Event::Contribution(b)).unwrap();
    assert_eq!(
        s.built.view.integrated_observations,
        (input().0.observations.len() + input().1.observations.len()) as u64
    );
    assert_eq!(s.built.view.portals.len(), 14);
    assert!(s.built.view.voxels.len() > 400);
    let before = view(&s);
    assert!(!s.apply(Event::Contribution(a.clone())).unwrap());
    assert_eq!(view(&s), before);
    let mut wrapped = a;
    wrapped.id = "phone-retransmission".into();
    s.apply(Event::Contribution(wrapped)).unwrap();
    assert_eq!(
        s.built.view.integrated_observations,
        (input().0.observations.len() + input().1.observations.len()) as u64
    );
    assert_eq!(
        serde_json::to_value(&s.built.view.voxels).unwrap(),
        before["voxels"]
    );
    s.apply(Event::Detection(d.clone())).unwrap();
    assert_eq!(s.built.view.objects.len(), 1);
    let p = s.built.view.objects[0].position;
    assert!(
        (p[0] + 3.65).abs() < 1e-9 && (p[1] - 0.9).abs() < 1e-9 && (p[2] - 2.15).abs() < 1e-9,
        "{p:?}"
    );
    assert!(!s.apply(Event::Detection(d)).unwrap());
}
#[test]
fn journal_restores_geometry_objects_and_deduplication() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("home.json");
    let (a, b, d) = input();
    let mut s = Store::open(&path).unwrap();
    s.apply(Event::Contribution(a.clone())).unwrap();
    s.apply(Event::Contribution(b)).unwrap();
    s.apply(Event::Detection(d)).unwrap();
    let expected = view(&s);
    drop(s);
    let mut recovered = Store::open(&path).unwrap();
    assert_eq!(view(&recovered), expected);
    assert!(!recovered.apply(Event::Contribution(a)).unwrap());
    assert_eq!(view(&recovered), expected);
}
#[test]
fn invalid_contributions_never_change_memory_or_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("home.json");
    let mut s = Store::open(&path).unwrap();
    let (a, b, _) = input();
    s.apply(Event::Contribution(a.clone())).unwrap();
    let expected = view(&s);
    let bytes = std::fs::read(&path).unwrap();
    for case in 0..7 {
        let mut bad = b.clone();
        bad.id = format!("bad-{case}");
        match case {
            0 => bad.peer = "stranger".into(),
            1 => bad.observations[0].clock.peer_id = "iphone".into(),
            2 => bad.observations[0].sensor_to_map.to_frame_id = "unknown".into(),
            3 => bad.observations[0].sensor_to_map.rotation_wxyz = [0.; 4],
            4 => bad.observations[0].hit_points_m = vec![[0., 0., 1.]; 1025],
            5 => bad.scenegraph.as_mut().unwrap().usda = "inconsistent".into(),
            _ => {
                bad = a.clone();
                bad.observations[0].hit_points_m[0][0] += 1.;
            }
        }
        assert!(s.apply(Event::Contribution(bad)).is_err(), "case {case}");
        assert_eq!(view(&s), expected);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
#[test]
fn persistence_failure_does_not_publish_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("home.json");
    let mut s = Store::open(&path).unwrap();
    let before = view(&s);
    std::fs::create_dir(&path).unwrap();
    assert!(s.apply(Event::Contribution(input().0)).is_err());
    assert_eq!(view(&s), before);
}
#[test]
fn missing_geometry_does_not_create_a_fake_object() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(dir.path().join("home.json")).unwrap();
    assert!(s.apply(Event::Detection(input().2)).is_err());
    assert!(s.built.view.objects.is_empty());
    assert!(
        serde_json::to_value(&s.built.view)
            .unwrap()
            .get("revision")
            .is_none()
    );
}
#[test]
fn portal_only_scenegraph_ingestion_adds_a_new_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(dir.path().join("home.json")).unwrap();
    let (mut a, _, _) = input();
    a.observations.clear();
    let snapshot = a.scenegraph.as_mut().unwrap();
    snapshot.scenegraph.anchors.insert(
        uuid::Uuid::from_u128(2).to_string(),
        portal("iphone-world", "B", [1., 1., 0.]),
    );
    *snapshot = auki_scenegraph::MapSnapshot::new(snapshot.scenegraph.clone()).unwrap();
    s.apply(Event::Contribution(a)).unwrap();
    assert_eq!(s.built.view.portals.len(), 15);
    assert_eq!(s.built.view.integrated_observations, 0);
}

#[test]
fn legacy_demo_is_migrated_to_domain_frame_without_changing_geometry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("home.json");
    let (a, b, mut d) = input();
    d.camera_to_map.to_frame_id = "home-world".into();
    let old = Journal {
        schema: 1,
        events: vec![
            Event::Contribution(a.clone()),
            Event::Contribution(b),
            Event::Detection(d),
        ],
    };
    std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.journal.schema, 2);
    assert_eq!(store.built.view.frame, spatial_home::model::FRAME);
    assert_eq!(store.built.view.objects.len(), 1);
    assert_eq!(store.built.view.portals.len(), 14);
    assert_eq!(
        store
            .built
            .portals
            .scenegraph
            .map
            .domain_reference
            .as_deref(),
        Some(spatial_home::domain_portals::DOMAIN)
    );
    for anchor in &store.built.view.portals {
        assert_eq!(anchor.pose_in_map.to_frame_id, FRAME);
    }
    assert!(!store.apply(Event::Contribution(a)).unwrap());
    assert!(!store.apply(Event::Detection(input().2)).unwrap());
    let saved: Journal = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved.schema, 2);
}

#[test]
fn illustrative_floor_covers_every_supplied_marker_and_upgrades_old_demo() {
    let room = spatial_home::synthetic_room::Room::load().unwrap();
    let domain = spatial_home::domain_portals::load().unwrap();
    for portal in domain.scenegraph.anchors.values() {
        let [x, y, z] = portal.pose_in_map.translation;
        assert_eq!(y, 0.);
        assert!(room.contains_floor(x, z));
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("home.json");
    let (mut a, mut b, mut d) = input();
    a.id = "iphone-scan-1".into();
    b.id = "bracketbot-scan-1".into();
    d.id = "book-detection-1".into();
    std::fs::write(
        &path,
        serde_json::to_vec(&Journal {
            schema: 2,
            events: vec![
                Event::Contribution(a),
                Event::Contribution(b),
                Event::Detection(d),
            ],
        })
        .unwrap(),
    )
    .unwrap();
    let store = Store::open(&path).unwrap();
    assert!(store.built.view.voxels.len() > 3000);
    assert_eq!(store.built.view.objects[0].detection_id, "book-room-v2");
    assert_eq!(
        store
            .built
            .view
            .portals
            .iter()
            .filter(|p| p.anchor_id.len() == 11)
            .count(),
        13
    );
}

#[test]
fn additional_runner_config_preserves_primary_and_rejects_duplicate_peers() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("peer.json");
    let mut config: serde_json::Value =
        serde_json::from_str(include_str!("../config/home-peer.example.json"))?;
    // Two robot runners of its own; the test does not depend on the example's robots.
    let runner = |peer_id: &str, ip: &str| serde_json::json!({
        "schema": "spatial-harness.home-connection/v1",
        "peer_id": peer_id,
        "domain_id": "b2881524-954d-4979-b715-240ed8f02807",
        "routes": [format!("/ip4/{ip}/tcp/4321")],
        "frame": "robot-local",
        "product_id": "canonical-scene"
    });
    config["runner_scene"] = runner("12D3KooWKogrsBMYLjBduXcXzefB7HruGYGxsqJPVYEvgmo1hJy5", "192.168.9.95");
    config["runner_scenes"] = serde_json::json!([{
        "name": "Second robot",
        "connection": runner("12D3KooWP7MFxucx3sPDvdYzXU8vBA1oRTmmx1ARGPxTRHQQYEu4", "192.168.9.87")
    }]);
    std::fs::write(&file, serde_json::to_vec(&config)?)?;
    let loaded = spatial_home::peer::PeerSettings::load(&file)?;
    assert_eq!(loaded.runner_scenes.len(), 1);
    assert_ne!(
        loaded.runner_scene.as_ref().unwrap().peer_id,
        loaded.runner_scenes[0].connection.peer_id
    );
    config["runner_scenes"][0]["connection"] = config["runner_scene"].clone();
    std::fs::write(&file, serde_json::to_vec(&config)?)?;
    assert!(spatial_home::peer::PeerSettings::load(&file).is_err());
    config.as_object_mut().unwrap().remove("runner_scenes");
    std::fs::write(&file, serde_json::to_vec(&config)?)?;
    assert!(
        spatial_home::peer::PeerSettings::load(&file)?
            .runner_scenes
            .is_empty()
    );
    Ok(())
}
