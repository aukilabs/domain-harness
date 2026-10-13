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
        let scenes = Arc::new(support::seeded_local_scenes(dir.path())?);
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
        let stroke=spatial_home::scene_paths::Stroke { id:uuid::Uuid::new_v4().to_string(),title:"Walk here".into(),color:"#ffca55".into(),width_m:0.035,segments:vec![vec![[0.,0.,0.],[1.,0.,1.]]] };
        scenes.paths.change("humanoid-hub-conference",&home.frame,&stroke.id,Some(stroke.clone()))?;
        let semantic_volume = spatial_home::semantic_volumes::Volume {
            id: uuid::Uuid::new_v4().to_string(), title: "Meeting area".into(),
            description: "Team gathering space".into(), center: [1., 1., 2.], size: [2., 2., 3.], yaw: 0.,
            semantic_type: "room".into(),
        };
        scenes.volumes.change("humanoid-hub-conference", &home.frame, None, Some(semantic_volume.clone()), &home.frame)?;
        home.validate(&pair.b)?;
        // Volume creation uses the same authenticated scene-scoped operation as notes.
        let volume = spatial_home::semantic_volumes::SemanticVolume {
            id: uuid::Uuid::new_v4().to_string(), name: "Kitchen".into(),
            semantic_type: "room".into(), position: [1., 0., 2.],
            dimensions: [2., 2.5, 3.], yaw: 0.5, description:None,
        };
        let request = spatial_home::mobile_notes::ChangeNote {
            request_id: uuid::Uuid::new_v4().to_string(), frame: home.frame.clone(),
            action: spatial_home::mobile_notes::NoteAction::PutVolume { volume: volume.clone() },
        };
        spatial_home::mobile_notes::send_change(&pair.b, &home, &request).await?;
        spatial_home::mobile_notes::send_change(&pair.b, &home, &request).await?;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        for publisher in &mut publishers { publisher.tick()?; }
        let volume_view = read(&pair.b, &home).await?;
        assert_eq!(volume_view["render"]["paths"]["items"][0]["id"],stroke.id);
        assert_eq!(volume_view["render"]["paths_product"]["product_id"],"local-scene:humanoid-hub-conference:paths");
        // A second peer can consume paths directly as an SDK Product.
        let client = auki_component_protocol::ComponentProtocolClient::new(pair.b.protocols());
        let reference = serde_json::from_value(volume_view["render"]["paths_product"].clone())?;
        let mut subscription = client.subscribe_product_exact(
            pair.a.peer_id(), pair.a.listen_addresses()[0].clone(), reference,
            auki_component_protocol::ObservationStart::LatestExisting,
            auki_components::BufferLimits::entries(1),
            |p: &spatial_home::scene_paths::Paths| serde_json::to_vec(p).unwrap().len(),
        ).await?;
        let Some(auki_component_protocol::RemoteObservationEvent::Observation(observation)) =
            tokio::time::timeout(Duration::from_secs(10), subscription.next()).await??
        else { panic!("missing retained paths Product"); };
        assert_eq!(observation.payload.frame, home.frame);
        assert_eq!(observation.payload.items, vec![stroke.clone()]);
        let mut extended=stroke.clone();extended.segments[0].push([2.,0.,1.]);
        scenes.paths.extend("humanoid-hub-conference",&home.frame,extended.clone())?;
        for publisher in &mut publishers {publisher.tick()?;}
        let Some(auki_component_protocol::RemoteObservationEvent::Observation(update)) =
            tokio::time::timeout(Duration::from_secs(10),subscription.next()).await??
        else {panic!("missing live stroke update");};
        assert_eq!(update.payload.items,vec![extended]);
        subscription.close().await?;
        assert!(read(&pair.b,&nacs).await?["render"]["paths"]["items"].as_array().unwrap().is_empty());
        assert_eq!(volume_view["render"]["volumes"].as_array().unwrap().len(), 2);
        assert!(volume_view["render"]["volumes"].as_array().unwrap().iter().any(|v|v["id"]==volume.id));
        assert!(read(&pair.b, &nacs).await?["render"]["volumes"].as_array().unwrap().is_empty());
        let saved=scenes.volumes.get("humanoid-hub-conference",&home.frame)?;
        assert!(saved.items.iter().any(|v|v.id==volume.id));
        spatial_home::mobile_notes::send_change(&pair.b,&home,&spatial_home::mobile_notes::ChangeNote{
            request_id:uuid::Uuid::new_v4().to_string(),frame:home.frame.clone(),
            action:spatial_home::mobile_notes::NoteAction::DeleteVolume{id:volume.id},
        }).await?;
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
        colors: None,
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
        let editor=scenes.note_editors.lock().unwrap()["humanoid-hub-conference"].clone();
        let mut picks=spatial_home::picking::Picks::default();
        let hit=editor.pick(&mut picks,[0.,2.,0.],[0.,-1.,0.])?;
        let contact=picks.contact(hit["hit"]["token"].as_str().unwrap())?;
        assert_eq!(contact.anchor.frame,home.frame);
        let browser_note=spatial_home::notes::TextNote{id:uuid::Uuid::new_v4().to_string(),text:"Browser placed".into(),details:Default::default(),position:contact.position,anchor_portal:String::new(),anchor_robot:None,height_m:0.,anchor_surface:Some(contact.anchor)};
        assert!(scenes.note_editors.lock().unwrap()["nacs"].create(browser_note.clone()).is_err());
        let created=editor.create(browser_note)?;
        assert_eq!(created.text,"Browser placed");
        assert_eq!(editor.change(&created.id,None)?.unwrap().id,created.id);
        let details=spatial_home::notes::NoteDetails {description:"Browser description".into(),..Default::default()};
        let edited=editor.change(&id,Some(("Browser edit".into(),Some(details))))?.unwrap();
        assert_eq!(edited.text,"Browser edit");
        assert_eq!(edited.details.description,"Browser description");
        assert!(scenes.note_editors.lock().unwrap()["nacs"].change(&id,Some(("Wrong scene".into(),None))).is_err());
        assert_eq!(editor.change(&id,None)?.unwrap().text,"Browser edit");
        let persisted:spatial_home::notes::Notes=serde_json::from_slice(&std::fs::read(dir.path().join("mobile-notes/humanoid-hub-conference.json"))?)?;
        assert!(!persisted.items.iter().any(|n|n.id==id));
        send_change(&pair.b,&home,&request(NoteAction::Delete { id })).await?;
        let pose = spatial_home::mobile_pose::Update {
            sample: Some(spatial_home::mobile_pose::Sample {
                session: session.clone(),
                timestamp_ns: 200,
                camera_configuration_id:None,
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
        assert_eq!(mapped["render"]["integrated_observations"], 1, "{mapped}");
        assert_eq!(mapped["render"]["frame"], home.frame);
        assert_eq!(mapped["render"]["semantic_volumes"]["items"][0], serde_json::to_value(&semantic_volume)?);
        let mut phone_volume=semantic_volume.mobile();
        phone_volume.name="iPhone edit".into();
        phone_volume.position=[3.,0.5,4.];
        phone_volume.description=None; // Existing iPhone wire format.
        let put=request(NoteAction::PutVolume{volume:phone_volume.clone()});
        assert!(send_change(&pair.b,&nacs,&put).await.is_err());
        send_change(&pair.b,&home,&put).await?;
        send_change(&pair.b,&home,&put).await?;
        let shared=scenes.volumes.get("humanoid-hub-conference",&home.frame)?;
        assert_eq!(shared.items.len(),1);
        assert_eq!(shared.items[0].title,"iPhone edit");
        assert_eq!(shared.items[0].center,[3.,1.5,4.]);
        assert_eq!(shared.items[0].description,"Team gathering space");
        let mut browser_edit=shared.items[0].clone();browser_edit.title="Home edit".into();
        scenes.volumes.change("humanoid-hub-conference",&home.frame,Some(&browser_edit.id),Some(browser_edit.clone()),&home.frame)?;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        for publisher in &mut publishers { publisher.tick()?; }
        let published=read(&pair.b,&home).await?;
        assert_eq!(published["render"]["volumes"][0]["name"],"Home edit");
        assert_eq!(published["render"]["semantic_volumes"]["items"][0]["id"],browser_edit.id);
        assert!(scenes.volumes.get("nacs",&nacs.frame)?.items.is_empty());
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
        assert!(untouched["render"]["semantic_volumes"]["items"].as_array().unwrap().is_empty());
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
        let editor=scenes.note_editors.lock().unwrap()["humanoid-hub-conference"].clone();
        assert!(!retained["render"]["voxels"].as_array().unwrap().is_empty());
        editor.reset_mapping(&editor.reference()?)?;
        assert!(spatial_home::mobile_depth::send(&pair.b,&home,&dense).await.is_err(),"Old calibration cannot repopulate a reset map");
        tokio::time::sleep(Duration::from_millis(1100)).await;
        for publisher in &mut publishers {publisher.tick()?;}
        let cleared=read(&pair.b,&home).await?;
        assert!(cleared["scenegraph"]["anchors"].as_object().unwrap().is_empty());
        assert!(cleared["render"]["voxels"].as_array().unwrap().is_empty());
        assert_eq!(cleared["render"]["semantic_volumes"],retained["render"]["semantic_volumes"]);
        assert_eq!(editor.get_note(&note.id)?,saved);
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
        assert_eq!(restored["render"]["semantic_volumes"]["items"][0], serde_json::to_value(&browser_edit)?);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_reports_a_new_portal_into_the_selected_scene() -> Result<()> {
    use spatial_home::local_mapper::{compose, identity, inverse};
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let scenes = Arc::new(support::seeded_local_scenes(dir.path())?);
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
        let reference = scenes.snapshot("humanoid-hub-conference")?;
        let portal = &reference.scenegraph.anchors["HNY3JGNLRAE"];
        let session = uuid::Uuid::new_v4().to_string();
        let session_frame = format!("phone-ar:{session}");
        let camera = format!("phone-camera:{session}");
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let home_from_session = auki_odometry::Pose {
            translation: [4., 0., 0.],
            rotation_xyzw: [0., q, 0., q],
        };
        let session_from_portal = compose(
            &inverse(&home_from_session)?,
            &from_rigid(&portal.pose_in_map),
        )?;
        let alignment = Alignment {
            timestamp_ns: 100,
            portal_id: portal.anchor_id.clone(),
            portal_to_session: rigid(
                &session_from_portal,
                &portal.pose_in_map.from_frame_id,
                &session_frame,
            ),
            session_to_home: rigid(&home_from_session, &session_frame, &home.frame),
        };
        let new_code = "NEWPORTAL01";
        let new_in_home = auki_odometry::Pose {
            translation: [2., 0., 1.],
            rotation_xyzw: [5_f64.to_radians().sin(),0.,0.,5_f64.to_radians().cos()],
        };
        let new_in_session = compose(&inverse(&home_from_session)?, &new_in_home)?;
        let new_local = spatial_home::mobile_portal::local_frame(new_code);
        let report = spatial_home::mobile_portal::Report {
            session: session.clone(),
            timestamp_ns: 250,
            code: new_code.into(),
            side_length_m: spatial_home::mobile_portal::DEFAULT_SIDE_M,
            portal_to_session: rigid(&new_in_session, &new_local, &session_frame),
            alignment: alignment.clone(),
            replace: false,
        };
        let mut forged = report.clone();
        forged.alignment.session_to_home.translation[0] += 1.;
        assert!(
            spatial_home::mobile_portal::send(&pair.b, &home, &forged)
                .await
                .is_err(),
            "Home checks the calibration Portal before trusting a new one"
        );
        assert!(
            spatial_home::mobile_portal::send(&pair.b, &nacs, &report)
                .await
                .is_err(),
            "cross-scene Portal report rejected"
        );
        assert!(spatial_home::mobile_portal::send(&pair.b, &home, &report).await?);
        assert!(
            spatial_home::mobile_portal::send(&pair.b, &home, &report)
                .await
                .is_err(),
            "a known Portal is not replaced"
        );
        let stored: auki_scenegraph::MapSnapshot = serde_json::from_slice(&std::fs::read(
            dir.path().join("humanoid-hub-conference.json"),
        )?)?;
        let added = &stored.scenegraph.anchors[new_code];
        assert!(added.pose_in_map.rotation_wxyz[0].abs()>1.-1e-9,"Home straightens a slightly tilted upright wall Portal");
        assert_eq!(added.side_length_m, 0.15);
        for (got, want) in added.pose_in_map.translation.iter().zip([2., 0., 1.]) {
            assert!((got - want).abs() < 1e-9, "Home composed the new Portal pose");
        }
        // Someone moved the printed target: the phone, aligned through another Portal, moves it in the scene.
        let moved_in_home = auki_odometry::Pose { translation: [3., 0., 1.], ..identity() };
        let mut moved = report.clone();
        moved.replace = true;
        moved.timestamp_ns = 270;
        moved.side_length_m = 0.5;
        moved.portal_to_session = rigid(&compose(&inverse(&home_from_session)?, &moved_in_home)?, &new_local, &session_frame);
        assert!(spatial_home::mobile_portal::send(&pair.b, &home, &moved).await?);
        let stored: auki_scenegraph::MapSnapshot = serde_json::from_slice(&std::fs::read(
            dir.path().join("humanoid-hub-conference.json"),
        )?)?;
        let relocated = &stored.scenegraph.anchors[new_code];
        assert!((relocated.pose_in_map.translation[0] - 3.).abs() < 1e-9, "the Portal is where the phone measured it");
        assert_eq!(relocated.side_length_m, 0.15, "a moved Portal keeps its printed size");
        let mut origin = moved.clone();
        origin.code = "HNY3JGNLRAE".into();
        origin.portal_to_session.from_frame_id = portal.pose_in_map.from_frame_id.clone();
        let mut by_itself = moved.clone();
        by_itself.alignment.portal_id = new_code.into();
        let mut unknown = moved.clone();
        unknown.code = "NOTINSCENE1".into();
        unknown.portal_to_session.from_frame_id = spatial_home::mobile_portal::local_frame("NOTINSCENE1");
        for (bad, why) in [(origin, "the origin Portal stays put"), (by_itself, "a Portal is not its own reference"),
                           (unknown, "only a Portal in the scene can be moved")] {
            assert!(spatial_home::mobile_portal::send(&pair.b, &home, &bad).await.is_err(), "{why}");
        }

        // A phone draws a path into its scene over the same link, and takes it out again.
        use spatial_home::scene_paths::{Add, Change, Remove, Stroke};
        let stroke = Stroke { id: uuid::Uuid::new_v4().to_string(), title: "Crayon path".into(), color: "#ffca55".into(),
            width_m: 0.035, segments: vec![vec![[0., 0., 0.], [0.1, 0., 0.]]] };
        let add = Change::Add(Add { frame: home.frame.clone(), stroke: stroke.clone() });
        assert!(spatial_home::scene_paths::send(&pair.b, &home, &add).await?);
        assert_eq!(scenes.paths.get("humanoid-hub-conference", &home.frame)?.items, vec![stroke.clone()]);
        assert!(scenes.paths.get("nacs", &nacs.frame)?.items.is_empty(), "the path is in the phone's scene only");
        assert!(spatial_home::scene_paths::send(&pair.b, &nacs, &add).await.is_err(), "a path drawn for another scene is refused");
        let wide = Change::Add(Add { frame: home.frame.clone(), stroke: Stroke { id: uuid::Uuid::new_v4().to_string(), width_m: 3., ..stroke.clone() } });
        assert!(spatial_home::scene_paths::send(&pair.b, &home, &wide).await.is_err(), "Home's own path rules still apply");
        let remove = Change::Remove(Remove { frame: home.frame.clone(), id: stroke.id.clone() });
        assert!(spatial_home::scene_paths::send(&pair.b, &home, &remove).await?);
        assert!(scenes.paths.get("humanoid-hub-conference", &home.frame)?.items.is_empty());
        assert!(spatial_home::scene_paths::send(&pair.b, &home, &remove).await.is_err(), "a path already gone cannot be removed");
        // Put it back so the rest of the test sees the scene it expects.
        moved.timestamp_ns = 280;
        moved.portal_to_session = rigid(&new_in_session, &new_local, &session_frame);
        assert!(spatial_home::mobile_portal::send(&pair.b, &home, &moved).await?);
        // Depth calibrated on the new Portal is accepted, and the scene still publishes.
        // A subsequent clean sighting uses the corrected reference, not the
        // deliberately tilted placement measurement used above.
        let new_in_session=compose(&inverse(&home_from_session)?,&auki_odometry::Pose {
            translation:[2.,0.,1.],..identity()
        })?;
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let phone = spatial_home::mobile_depth::Publisher::new(&products, &session)?;
        let sample = Sample {
            session: session.clone(),
            timestamp_ns: 300,
            camera_to_session: rigid(
                &auki_odometry::Pose {
                    translation: [0., 1., 0.],
                    ..identity()
                },
                &camera,
                &session_frame,
            ),
            alignment: Alignment {
                timestamp_ns: 260,
                portal_id: new_code.into(),
                portal_to_session: rigid(&new_in_session, &new_local, &session_frame),
                session_to_home: rigid(&home_from_session, &session_frame, &home.frame),
            },
            points: vec![[0., 0., -1.5]],
            colors: None,
        };
        assert!(spatial_home::mobile_depth::send(&pair.b, &home, &phone.publish(sample)?).await?);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        for publisher in &mut publishers {
            publisher.tick()?;
        }
        let grown = read(&pair.b, &home).await?;
        assert_eq!(grown["scenegraph"]["anchors"].as_object().unwrap().len(), 2);
        assert!(grown["scenegraph"]["anchors"].get(new_code).is_some());
        assert_eq!(grown["render"]["integrated_observations"], 1);
        // Joystick relay: the phone posts for one robot; that robot reads it back
        // with Home's own measure of its age. Here one peer plays both roles.
        let me = pair.b.peer_id().to_string();
        let stick = spatial_home::mobile_drive::Command {
            session: uuid::Uuid::new_v4().to_string(),
            sequence: 1,
            target_peer_id: me.clone(),
            forward: 0.5,
            turn: -0.25,
        };
        assert!(spatial_home::mobile_drive::send(&pair.b, &home, &stick).await?);
        assert!(
            spatial_home::mobile_drive::send(&pair.b, &nacs, &spatial_home::mobile_drive::Command {
                forward: 2., ..stick.clone()
            }).await.is_err(),
            "out-of-range stick rejected"
        );
        let mut reader = spatial_home::mobile_drive::Reader::default();
        let (seen, _) = reader.read(&pair.b, &home).await?;
        let command = seen.command.expect("fresh command for this robot");
        assert_eq!((command.driver_peer_id, command.forward, command.turn), (me, 0.5, -0.25));
        assert!(command.age_ms < 1000);
        let (other_scene, _) = reader.read(&pair.b, &nacs).await?;
        assert!(other_scene.command.is_none(), "commands do not cross scenes");
        tokio::time::sleep(spatial_home::mobile_drive::MAX_AGE).await;
        assert!(reader.read(&pair.b, &home).await?.0.command.is_none(), "silence stops the robot");
        // Home also pushes each accepted command, so a robot need not poll.
        let mut pushed = spatial_home::mobile_drive::subscribe(&pair.b, &home).await?;
        let robot = pair.b.peer_id().to_string();
        let held = spatial_home::mobile_drive::Command {
            session: uuid::Uuid::new_v4().to_string(),
            sequence: 1,
            target_peer_id: robot.clone(),
            forward: 0.25,
            turn: 0.5,
        };
        assert!(spatial_home::mobile_drive::send(&pair.b, &home, &held).await?);
        match tokio::time::timeout(Duration::from_secs(3), pushed.next()).await?? {
            Some(auki_component_protocol::RemoteObservationEvent::Observation(o)) => {
                assert_eq!(
                    (o.payload.driver_peer_id.as_str(), o.payload.target_peer_id.as_str(), o.payload.forward, o.payload.turn),
                    (robot.as_str(), robot.as_str(), 0.25, 0.5)
                );
            }
            _ => panic!("expected the pushed command"),
        }
        pushed.close().await?;
        // A restart keeps the reported Portal.
        let reloaded = LocalScenes::load(dir.path())?;
        assert_eq!(reloaded.snapshot("humanoid-hub-conference")?.scenegraph.anchors.len(), 2);
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
