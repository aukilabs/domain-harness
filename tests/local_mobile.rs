mod support;
use anyhow::Result;
use spatial_home::{
    local_scenes::LocalScenes,
    mobile_depth::{Alignment, Sample},
    network::{HomeConnection, ScenePublisher},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
fn from_rigid(p: &auki_scenegraph::RigidTransform) -> auki_odometry::Pose {
    let [w, x, y, z] = p.rotation_wxyz;
    auki_odometry::Pose {
        translation: p.translation,
        rotation_xyzw: [x, y, z, w],
    }
}
fn rigid(p: &auki_odometry::Pose, from: &str, to: &str) -> auki_scenegraph::RigidTransform {
    let [x, y, z, w] = p.rotation_xyzw;
    auki_scenegraph::RigidTransform {
        from_frame_id: from.into(),
        to_frame_id: to.into(),
        translation: p.translation,
        rotation_wxyz: [w, x, y, z],
    }
}

async fn read(peer: &auki_sdk::AukiPeer, home: &HomeConnection) -> Result<serde_json::Value> {
    let mut value = None;
    tokio::time::timeout(
        Duration::from_secs(10),
        spatial_home::mobile::receive_portals(
            peer,
            home,
            &CancellationToken::new(),
            Duration::from_millis(50),
            |v| value = Some(v),
        ),
    )
    .await??;
    Ok(value.expect("retained scene"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selected_scene_phone_calibration_depth_notes_and_presence_are_isolated() -> Result<()> {
    use spatial_home::local_mapper::{compose, identity, inverse};
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let scenes = Arc::new(LocalScenes::load(dir.path())?);
        let original_nacs = std::fs::read(dir.path().join("nacs.json"))?;
        let original_conference = std::fs::read(dir.path().join("humanoid-hub-conference.json"))?;
        let store = spatial_home::model::Store::open_canonical(dir.path().join("xmart.journal"))?;
        let mut server = ScenePublisher::mount(&pair.a)?;
        server.publish(&store.built)?;
        let owner = pair.a.peer_id().to_string();
        let clock = spatial_home::model::clock_entry(&owner, &uuid::Uuid::new_v4().to_string());
        let mut publishers = spatial_home::local_mobile::Publisher::mount_all(
            &server.runtime,
            &server.endpoint,
            scenes.clone(),
            clock.clone(),
            Instant::now(),
        )?;
        let home = scenes.connection("humanoid-hub-conference", HomeConnection::direct(&pair.a))?;
        let nacs = scenes.connection("nacs", HomeConnection::direct(&pair.a))?;
        home.validate(&pair.b)?;
        let runner_reference = spatial_home::network::fetch_home_portals(&pair.b, &home).await?;
        assert_eq!(runner_reference.scenegraph.map.frame.id, home.frame);
        assert_eq!(runner_reference.scenegraph.anchors.len(), 1);
        assert!(runner_reference.scenegraph.anchors.contains_key("HNY3JGNLRAE"));
        let default_reference = spatial_home::network::fetch_home_portals(&pair.b, &HomeConnection::direct(&pair.a)).await?;
        assert_eq!(default_reference.scenegraph.map.frame.id, spatial_home::model::FRAME);
        let initial = read(&pair.b, &home).await?;
        assert!(
            initial["scenegraph"]["anchors"]
                .get("HNY3JGNLRAE")
                .is_some()
        );
        assert_eq!(
            initial["scenegraph"]["anchors"].as_object().unwrap().len(),
            1
        );
        assert_eq!(initial["render"]["integrated_observations"], 0);
        assert!(
            initial["render"]["voxels"].as_array().unwrap().is_empty(),
            "no manufactured sensor evidence"
        );
        let reference = scenes.snapshot("humanoid-hub-conference")?;
        let portal = &reference.scenegraph.anchors["HNY3JGNLRAE"];
        let session = uuid::Uuid::new_v4().to_string();
        let session_frame = format!("phone-ar:{session}");
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let home_from_session = auki_odometry::Pose {
            translation: [4., 0., 0.],
            rotation_xyzw: [0., q, 0., q],
        };
        let session_from_portal = compose(
            &inverse(&home_from_session)?,
            &from_rigid(&portal.pose_in_map),
        )?;
        let camera = format!("phone-camera:{session}");
        let sample = Sample {
            session: session.clone(),
            timestamp_ns: 200,
            camera_to_session: rigid(
                &auki_odometry::Pose {
                    translation: [0., 1., 0.],
                    ..identity()
                },
                &camera,
                &session_frame,
            ),
            alignment: Alignment {
                timestamp_ns: 100,
                portal_id: portal.anchor_id.clone(),
                portal_to_session: rigid(
                    &session_from_portal,
                    &portal.pose_in_map.from_frame_id,
                    &session_frame,
                ),
                session_to_home: rigid(&home_from_session, &session_frame, &home.frame),
            },
            points: vec![[0., 0., -1.]],
        };
        assert_ne!(sample.alignment.portal_to_session.translation, [0.; 3]);
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let phone = spatial_home::mobile_depth::Publisher::new(&products, &session)?;
        let envelope = phone.publish(sample)?;
        assert!(spatial_home::mobile_depth::send(&pair.b, &home, &envelope).await?);
        assert!(
            !spatial_home::mobile_depth::send(&pair.b, &home, &envelope).await?,
            "duplicate ignored"
        );
        assert!(
            spatial_home::mobile_depth::send(&pair.b, &nacs, &envelope)
                .await
                .is_err(),
            "cross-scene depth rejected"
        );
        let mut forged = envelope.clone();
        forged.sample.alignment.session_to_home.translation[0] += 1.;
        assert!(
            spatial_home::mobile_depth::send(&pair.b, &home, &forged)
                .await
                .is_err(),
            "Home checks observed portal against claimed transform"
        );
        let note = spatial_home::mobile_notes::PlaceNote {
            id: uuid::Uuid::new_v4().to_string(),
            frame: home.frame.clone(),
            origin: [1., 1., 1.],
            direction: [0., -1., 0.],
            text: "Conference floor".into(),
        };
        assert!(
            spatial_home::mobile_notes::send(&pair.b, &nacs, &note)
                .await
                .is_err()
        );
        let saved = spatial_home::mobile_notes::send(&pair.b, &home, &note).await?;
        assert_eq!(saved.position, [1., 0., 1.]);
        assert_eq!(
            saved,
            spatial_home::mobile_notes::send(&pair.b, &home, &note).await?,
            "retry is idempotent"
        );
        // Phone-measured surfaces need not already exist in Home's voxel map.
        use spatial_home::mobile_notes::{ChangeNote, NoteAction, send_change};
        let id = uuid::Uuid::new_v4().to_string();
        let request = |action| ChangeNote { request_id: uuid::Uuid::new_v4().to_string(), frame: home.frame.clone(), action };
        let place = request(NoteAction::Place { id: id.clone(), origin: [0.,1.,0.], position: [0.,1.,-2.], text: "Note".into() });
        assert!(send_change(&pair.b,&nacs,&place).await.is_err());
        let created = send_change(&pair.b,&home,&place).await?.note.unwrap();
        assert_eq!(created.position,[0.,1.,-2.]);
        assert_eq!(send_change(&pair.b,&home,&place).await?.note.unwrap(),created);
        let edit = request(NoteAction::Edit { id: id.clone(),text: "Door".into() });
        assert_eq!(send_change(&pair.b,&home,&edit).await?.note.unwrap().text,"Door");
        let delete = request(NoteAction::Delete { id: id.clone() });
        let removed = send_change(&pair.b,&home,&delete).await?;
        assert!(removed.deleted);
        let deleted = removed.note.unwrap();
        assert_eq!(send_change(&pair.b,&home,&delete).await?.note,Some(deleted.clone()),"Operation retries return the original result for Undo");
        send_change(&pair.b,&home,&request(NoteAction::Restore { note: deleted })).await?;
        send_change(&pair.b,&home,&request(NoteAction::Delete { id })).await?;
        let pose = spatial_home::mobile_pose::Update {
            sample: Some(spatial_home::mobile_pose::Sample {
                session: session.clone(),
                timestamp_ns: 200,
                pose: rigid(&home_from_session, &camera, &home.frame),
            }),
            status: None,
        };
        spatial_home::mobile_pose::send(&pair.b, &home, &pose).await?;
        // Presence is immediately readable, without waiting for the 1 Hz scene tick.
        let live = scenes.presence("humanoid-hub-conference")?;
        assert_eq!(live["poses"][0]["capture_timestamp_ns"], 200);
        assert!(scenes.presence("nacs")?["poses"].as_array().unwrap().is_empty());
        assert!(
            spatial_home::mobile_pose::send(&pair.b, &nacs, &pose)
                .await
                .is_err()
        );
        tokio::time::sleep(Duration::from_millis(1100)).await;
        for publisher in &mut publishers {
            publisher.tick()?;
        }
        let mapped = read(&pair.b, &home).await?;
        assert_eq!(mapped["render"]["integrated_observations"], 1);
        assert_eq!(mapped["render"]["frame"], home.frame);
        assert!(
            mapped["render"]["voxels"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| {
                    let c = &v["center"];
                    (c[0].as_f64().unwrap() - 3.).abs() < 0.101
                        && (c[1].as_f64().unwrap() - 1.).abs() < 0.101
                        && c[2].as_f64().unwrap().abs() < 0.101
                }),
            "camera-local [0,0,-1] transformed exactly once to Home [3,1,0]"
        );
        assert!(
            mapped["render"]["notes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["id"] == note.id)
        );
        assert!(
            mapped["render"]["notes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["phone_pose"].is_object())
        );
        let untouched = read(&pair.b, &nacs).await?;
        assert!(untouched["render"]["voxels"].as_array().unwrap().is_empty());
        assert!(untouched["render"]["notes"].as_array().unwrap().is_empty());
        let xmart =
            spatial_home::network::fetch_home_scene(&pair.b, &HomeConnection::direct(&pair.a))
                .await?;
        assert_eq!(xmart.frame, spatial_home::model::FRAME);
        assert_eq!(xmart.portals.scenegraph.anchors.len(), 13);
        assert_eq!(std::fs::read(dir.path().join("nacs.json"))?, original_nacs);
        assert_eq!(
            std::fs::read(dir.path().join("humanoid-hub-conference.json"))?,
            original_conference
        );
        assert_eq!(
            scenes.view("humanoid-hub-conference")?["integrated_observations"],
            1
        );
        let mut mismatched = home.clone();
        mismatched.product_id = nacs.product_id.clone();
        assert!(mismatched.validate(&pair.b).is_err());
        let mut wrong_domain = home.clone();
        wrong_domain.domain_id = uuid::Uuid::new_v4().to_string();
        assert!(read(&pair.b, &wrong_domain).await.is_err());
        // Real phone captures exceed both the mapper's point limit and its
        // ray-work budget at long range. They must be sampled, not discarded.
        let mut dense = envelope.clone();
        dense.sequence += 1;
        dense.sample.timestamp_ns += 100;
        dense.sample.points = (0..2048)
            .map(|i| [(i % 64) as f64 / 32. - 1., (i / 64) as f64 / 32., -5.])
            .collect();
        assert!(spatial_home::mobile_depth::send(&pair.b, &home, &dense).await?);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        for publisher in &mut publishers {
            publisher.tick()?;
        }
        let dense_map = read(&pair.b, &home).await?;
        assert_eq!(
            dense_map["render"]["integrated_observations"], 2,
            "dense capture integrates within SDK work limits"
        );
        phone.close().await?;
        products.close().await?;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        for publisher in &mut publishers {
            publisher.tick()?;
        }
        let retained = read(&pair.b, &home).await?;
        assert_eq!(
            retained["render"]["voxels"], dense_map["render"]["voxels"],
            "no decay when the phone stops publishing"
        );
        drop(publishers);
        server.close().await?;
        // Home retains authored notes across restarts, but does not resurrect a
        // previous run's sensor map as fresh observations.
        let server = ScenePublisher::mount(&pair.a)?;
        let restarted = Arc::new(LocalScenes::load(dir.path())?);
        let publishers = spatial_home::local_mobile::Publisher::mount_all(
            &server.runtime,
            &server.endpoint,
            restarted,
            clock,
            Instant::now(),
        )?;
        let restored = read(&pair.b, &home).await?;
        assert_eq!(restored["render"]["notes"].as_array().unwrap().len(), 1);
        assert_eq!(restored["render"]["notes"][0]["id"], note.id);
        assert!(restored["render"]["voxels"].as_array().unwrap().is_empty());
        drop(publishers);
        server.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[test]
fn equal_axes_do_not_make_independent_origins_interchangeable() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scenes = LocalScenes::load(dir.path())?;
    let conference = scenes.snapshot("humanoid-hub-conference")?;
    let nacs = &scenes.snapshot("nacs")?.scenegraph.map.frame.id;
    assert!(spatial_home::conventions::portals(conference, nacs).is_err());
    assert!(spatial_home::conventions::portals(conference, spatial_home::model::FRAME).is_err());
    let a = spatial_home::conventions::registry("home", &conference.scenegraph.map.frame.id)?;
    let b = spatial_home::conventions::registry("home", nacs)?;
    assert!(spatial_home::conventions::point([0.; 3], &a, &b).is_err());
    Ok(())
}
