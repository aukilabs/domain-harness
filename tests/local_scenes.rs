mod support;
use anyhow::Result;
use auki_scenegraph::MapSnapshot;
use spatial_home::{local_scenes::LocalScenes, model};

#[test]
fn scene_settings_rename_without_reframing_and_do_not_resurrect_deleted_scenes() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scenes = support::seeded_local_scenes(dir.path())?;
    let nacs_before = scenes.snapshot("nacs")?.scenegraph.clone();
    let conference_before = scenes.snapshot("humanoid-hub-conference")?.scenegraph.clone();
    let bb_bytes = std::fs::read(dir.path().join("bb-lab.json"))?;
    let retained_notes = br#"{"fixture":"untouched"}"#;
    for subdir in ["mobile-notes", "semantic-volumes", "paths"] {
        std::fs::create_dir_all(dir.path().join(subdir))?;
        std::fs::write(dir.path().join(subdir).join("bb-lab.json"), retained_notes)?;
        std::fs::write(dir.path().join(subdir).join("humanoid-hub-conference.json"), retained_notes)?;
    }
    std::fs::create_dir_all(dir.path().join(".settings"))?;
    std::fs::write(dir.path().join(".settings/catalog.json"), br#"{
        "delete_reference":true,"deleted":["bb-lab"],
        "names":{"nacs":"My apartment","humanoid-hub-conference":"1 Arkansas"}
    }"#)?;
    for _ in 0..2 {
        let loaded = LocalScenes::load(dir.path())?;
        assert!(!loaded.reference_enabled);
        assert_eq!(loaded.snapshots.len(), scenes.snapshots.len() - 1);
        assert!(loaded.snapshot("bb-lab").is_err());
        assert!(!dir.path().join("bb-lab.json").exists());
        for (id, old, name) in [("nacs", &nacs_before, "My apartment"),
            ("humanoid-hub-conference", &conference_before, "1 Arkansas")] {
            let mut expected = old.clone();
            expected.map.name = Some(name.into());
            let current = loaded.snapshot(id)?;
            current.validate()?;
            assert_eq!(serde_json::to_value(&current.scenegraph)?, serde_json::to_value(expected)?);
            assert!(loaded.rows().iter().any(|row|row["id"] == format!("local:{id}") && row["name"] == name));
        }
        assert_eq!(loaded.preferred_id().as_deref(), Some("local:humanoid-hub-conference"));
        assert_eq!(std::fs::read(dir.path().join(".deleted/bb-lab.json"))?, bb_bytes);
        for subdir in ["mobile-notes", "semantic-volumes", "paths"] {
            assert_eq!(std::fs::read(dir.path().join(".deleted").join(subdir).join("bb-lab.json"))?, retained_notes);
            assert!(!dir.path().join(subdir).join("bb-lab.json").exists());
            assert_eq!(std::fs::read(dir.path().join(subdir).join("humanoid-hub-conference.json"))?, retained_notes);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retired_reference_keeps_independent_scene_products_and_services() -> Result<()> {
    use auki_component_protocol::{CatalogResponse, ComponentProtocolClient};
    use spatial_home::network::{ScenePublisher, SCENE_PRODUCT};
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir_all(dir.path().join(".settings"))?;
        std::fs::write(dir.path().join(".settings/catalog.json"), br#"{
            "delete_reference":true,"deleted":["bb-lab"],
            "names":{"nacs":"My apartment","humanoid-hub-conference":"1 Arkansas"}
        }"#)?;
        let scenes = std::sync::Arc::new(LocalScenes::load(dir.path())?);
        let mut publisher = ScenePublisher::mount_without_reference(&pair.a)?;
        let clock = model::clock_entry(&pair.a.peer_id().to_string(), "local-scenes-test");
        let started = std::time::Instant::now();
        let components = scenes.mount(&publisher.runtime, &publisher.endpoint, model::clock_ref(&clock), started)?;
        let mut mobiles = spatial_home::local_mobile::Publisher::mount_all(
            &publisher.runtime, &publisher.endpoint, scenes.clone(), clock, started)?;
        for mobile in &mut mobiles { mobile.tick()?; }
        let client = ComponentProtocolClient::new(pair.b.protocols());
        let CatalogResponse::Snapshot { snapshot } = client.catalog_exact(
            pair.a.peer_id(), pair.a.listen_addresses()[0].clone(), None).await? else {
            panic!("Catalog unavailable")
        };
        for product in &snapshot.products {
            let id = &product.manifest.product_id;
            assert_ne!(id, SCENE_PRODUCT);
            assert_ne!(id, "home-live-scene");
            assert_ne!(id, "accepted-mapping-inputs");
            assert!(!id.contains("bb-lab"));
        }
        for id in ["nacs", "humanoid-hub-conference"] {
            assert!(snapshot.products.iter().any(|p|p.manifest.product_id == format!("local-scene:{id}:mobile")));
            for base in ["home-annotations", "home-phone-presence", "home-robot-presence"] {
                assert!(snapshot.components.iter().any(|c|c.manifest.component_id == format!("{base}:{id}")), "Missing {base}:{id}");
                assert!(!snapshot.components.iter().any(|c|c.manifest.component_id == base));
            }
            let connection = scenes.connection(id, spatial_home::network::HomeConnection::direct(&pair.a))?;
            connection.validate_scene_contract()?;
            assert_eq!(connection.local_scene_id(), Some(id));
            assert_eq!(scenes.view(id)?["environment_name"], scenes.snapshot(id)?.scenegraph.map.name.as_deref().unwrap());
        }
        let legacy = model::Store::open_canonical(dir.path().join("legacy.journal"))?;
        assert!(publisher.publish(&legacy.built).is_err());
        assert!(publisher.publish_inputs(&[]).is_err());
        drop(mobiles);
        drop(components);
        publisher.close().await?;
        Ok::<_, anyhow::Error>(())
    }.await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[test]
fn scene_settings_reject_unsafe_or_conflicting_targets_before_archiving() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let _scenes = LocalScenes::load(dir.path())?;
    std::fs::create_dir_all(dir.path().join(".settings"))?;
    for invalid in [r#"{"deleted":["../nacs"]}"#,
        r#"{"deleted":["bb-lab"],"names":{"bb-lab":"Still here"}}"#,
        r#"{"names":{"nacs":" "}}"#] {
        std::fs::write(dir.path().join(".settings/catalog.json"), invalid)?;
        assert!(LocalScenes::load(dir.path()).is_err());
        assert!(dir.path().join("bb-lab.json").is_file());
    }
    Ok(())
}

#[test]
fn saved_scenes_are_independent_and_existing_data_is_authoritative() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scenes = support::seeded_local_scenes(dir.path())?;
    let nacs = scenes.snapshot("nacs")?;
    assert_eq!(nacs.scenegraph.map.name.as_deref(), Some("NACS"));
    assert_ne!(nacs.scenegraph.map.frame.id, model::FRAME);
    let portal = &nacs.scenegraph.anchors["RG3E3LUITXC"];
    assert_eq!(portal.side_length_m, 0.15);
    assert_eq!(portal.pose_in_map.translation, [0.; 3]);
    let q = std::f64::consts::FRAC_1_SQRT_2;
    assert_eq!(portal.pose_in_map.rotation_wxyz, [q, -q, 0., 0.]);
    let [w, x, y, z] = portal.pose_in_map.rotation_wxyz;
    let pose = auki_odometry::Pose {
        translation: portal.pose_in_map.translation,
        rotation_xyzw: [x, y, z, w],
    };
    let face = spatial_home::local_mapper::compose(
        &pose,
        &auki_odometry::Pose {
            translation: [0., 0., 1.],
            ..spatial_home::local_mapper::identity()
        },
    )?;
    assert!(
        (face.translation[1] - 1.).abs() < 1e-12,
        "Printed face points upward"
    );
    let landmarks = spatial_home::landmarks::portal_map(nacs, None, &Default::default())?;
    for id in landmarks.landmarks.keys() {
        assert!(
            landmarks.position_in_map(id)?[1].abs() < 1e-12,
            "All four corners lie on floor"
        );
    }
    assert_eq!(scenes.view("nacs")?["portals"].as_array().unwrap().len(), 1);
    assert!(scenes.snapshot("missing").is_err());
    assert!(scenes.snapshot("../nacs").is_err());
    assert_eq!(
        spatial_home::domain_portals::load()?
            .scenegraph
            .anchors
            .len(),
        13
    );

    let mut graph = nacs.scenegraph.clone();
    graph.map.name = Some("Saved NACS name".into());
    let saved = MapSnapshot::new(graph.clone())?;
    let bytes = serde_json::to_vec(&saved)?;
    std::fs::write(dir.path().join("nacs.json"), &bytes)?;
    graph.map.map_id = "third-site".into();
    graph.map.name = Some("Third site".into());
    graph.map.frame.id = "third-site-frame".into();
    graph.anchors.clear();
    std::fs::write(
        dir.path().join("third-site.json"),
        serde_json::to_vec(&MapSnapshot::new(graph)?)?,
    )?;
    let restored = LocalScenes::load(dir.path())?;
    assert_eq!(restored.rows().len(), scenes.rows().len() + 1);
    assert_eq!(
        restored
            .snapshot("third-site")?
            .scenegraph
            .map
            .name
            .as_deref(),
        Some("Third site")
    );
    assert_eq!(std::fs::read(dir.path().join("nacs.json"))?, bytes);
    std::fs::write(dir.path().join("broken.json"), b"not a scenegraph")?;
    assert!(LocalScenes::load(dir.path()).is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saved_map_is_a_real_discoverable_sdk_product() -> Result<()> {
    use auki_component_protocol::ObservationStart;
    use auki_component_protocol::{
        CatalogResponse, ComponentProtocolClient, ComponentProtocolEndpoint, RemoteObservationEvent,
    };
    use auki_components::{BufferLimits, ComponentRuntime, ProductReference};
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let scenes = LocalScenes::load(dir.path())?;
        let owner = pair.a.peer_id().to_string();
        let runtime = ComponentRuntime::new(owner.clone());
        let endpoint = ComponentProtocolEndpoint::mount(pair.a.protocols(), runtime.clone())?;
        let components = scenes.mount(
            &runtime,
            &endpoint,
            model::clock_ref(&model::clock_entry(&owner, "test")),
            std::time::Instant::now(),
        )?;
        let client = ComponentProtocolClient::new(pair.b.protocols());
        let route = pair.a.listen_addresses()[0].clone();
        let CatalogResponse::Snapshot { snapshot } = client
            .catalog_exact(pair.a.peer_id(), route.clone(), None)
            .await?
        else {
            panic!("no catalog")
        };
        assert_eq!(components.len(), scenes.snapshots.len());
        for (id, component) in scenes.snapshots.keys().zip(&components) {
            let expected = component.product().reference();
            let entry = snapshot
                .products
                .iter()
                .find(|p| p.manifest.product_id == expected.product_id)
                .expect("native Map Product in SDK catalog");
            let mut subscription = client
                .subscribe_product_exact(
                    pair.a.peer_id(),
                    route.clone(),
                    ProductReference {
                        peer_id: owner.clone(),
                        product_id: entry.manifest.product_id.clone(),
                        manifest_hash: entry.manifest_hash.clone(),
                    },
                    ObservationStart::LatestExisting,
                    BufferLimits {
                        max_entries: Some(1),
                        max_bytes: Some(8 * 1024 * 1024),
                        target_duration: None,
                    },
                    |s: &MapSnapshot| serde_json::to_vec(s).unwrap().len(),
                )
                .await?;
            let Some(RemoteObservationEvent::Observation(observation)) =
                tokio::time::timeout(std::time::Duration::from_secs(10), subscription.next())
                    .await??
            else {
                panic!("no snapshot")
            };
            observation.payload.validate()?;
            assert_eq!(
                serde_json::to_value(&*observation.payload)?,
                serde_json::to_value(scenes.snapshot(id)?)?
            );
            subscription.close().await?;
        }
        drop(components);
        endpoint.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result?;
    cleanup?;
    Ok(())
}

#[test]
fn conference_origin_registers_an_independent_phone_session() -> Result<()> {
    use auki_odometry::Pose;
    use spatial_home::local_mapper::{compose, identity, inverse};
    let dir = tempfile::tempdir()?;
    let scenes = support::seeded_local_scenes(dir.path())?;
    assert_eq!(
        scenes.preferred_id().as_deref(),
        Some("local:humanoid-hub-conference")
    );
    let reference = scenes.snapshot("humanoid-hub-conference")?;
    assert_eq!(
        reference.scenegraph.map.name.as_deref(),
        Some("Humanoid Hub Conference")
    );
    assert_ne!(reference.scenegraph.map.frame.id, model::FRAME);
    assert_ne!(
        reference.scenegraph.map.frame.id,
        scenes.snapshot("nacs")?.scenegraph.map.frame.id
    );
    assert!(reference.scenegraph.map.domain_reference.is_none());
    assert_eq!(reference.scenegraph.anchors.len(), 1);
    let portal = &reference.scenegraph.anchors["HNY3JGNLRAE"];
    assert_eq!(portal.side_length_m, 0.15);
    assert_eq!(portal.pose_in_map.translation, [0.; 3]);
    let [w, x, y, z] = portal.pose_in_map.rotation_wxyz;
    let home_from_portal = Pose {
        translation: portal.pose_in_map.translation,
        rotation_xyzw: [x, y, z, w],
    };
    let normal = compose(
        &home_from_portal,
        &Pose {
            translation: [0., 0., 1.],
            ..identity()
        },
    )?;
    assert!((normal.translation[1] - 1.).abs() < 1e-12);
    let landmarks = spatial_home::landmarks::portal_map(reference, None, &Default::default())?;
    for id in landmarks.landmarks.keys() {
        assert!(landmarks.position_in_map(id)?[1].abs() < 1e-12);
    }
    // The AR session has its own origin and heading. A captured Portal is not
    // assigned the Home origin before the frame registration is computed.
    let q = std::f64::consts::FRAC_1_SQRT_2;
    let phone_from_home = Pose {
        translation: [2., 1., -3.],
        rotation_xyzw: [0., q, 0., q],
    };
    let phone_from_portal = compose(&phone_from_home, &home_from_portal)?;
    assert_ne!(phone_from_portal.translation, [0.; 3]);
    let home_from_phone = compose(&home_from_portal, &inverse(&phone_from_portal)?)?;
    let recovered = compose(&home_from_phone, &phone_from_portal)?;
    for (actual, expected) in recovered
        .translation
        .iter()
        .zip(home_from_portal.translation)
    {
        assert!((actual - expected).abs() < 1e-12);
    }
    // A point in front of the Portal maps above the Home floor, not below it.
    let phone_point = compose(
        &phone_from_portal,
        &Pose {
            translation: [0., 0., 1.],
            ..identity()
        },
    )?;
    let home_point = compose(&home_from_phone, &phone_point)?;
    assert!(home_point.translation[0].abs() < 1e-12);
    assert!((home_point.translation[1] - 1.).abs() < 1e-12);
    assert!(home_point.translation[2].abs() < 1e-12);
    Ok(())
}
