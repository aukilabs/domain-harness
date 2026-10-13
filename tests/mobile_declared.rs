mod support;
use anyhow::Result;
use spatial_home::{
    device_catalogs::fetch,
    mobile_declared::{self, Publisher},
    mobile_products::MobileProducts,
};

fn product<'a>(catalog: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
    catalog["catalog"]["products"].as_array().unwrap().iter().find(|p| p["manifest"]["product_id"] == id)
}

/// Any peer can ask another "do you know how your space maps into mine?" from
/// its Catalog alone, under the one transform profile, and finds depth it can
/// place without being sent a pose with every capture.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_declares_the_transform_it_knows_and_depth_for_another_peer_to_place() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let phone = pair.b.peer_id().to_string();
        let home = pair.a.peer_id().to_string();
        let dir = tempfile::tempdir()?;
        let scenes = support::seeded_local_scenes(dir.path())?;
        let scene_frame = scenes.snapshots["humanoid-hub-conference"].scenegraph.map.frame.id.clone();
        let products = MobileProducts::mount(&pair.b)?;
        let routes: Vec<_> = pair.b.listen_addresses().iter().map(ToString::to_string).collect();
        let session = uuid::Uuid::new_v4().to_string();
        let to_scene = |x: f64| auki_scenegraph::RigidTransform {
            from_frame_id: format!("phone-ar:{session}"),
            to_frame_id: scene_frame.clone(),
            translation: [x, 0., 2.],
            rotation_wxyz: [1.0004, 0., 0., 0.],
        };
        let mut publisher = Publisher::new(&products, &session, &home, &scene_frame)?;
        assert!(publisher.serves(&session, &home, &scene_frame));
        assert!(!publisher.serves(&session, &home, "local-scene:nacs:portal-RG3E3LUITXC"), "another scene is another pair of frames");

        // Nothing is claimed before there is an alignment.
        let before = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert!(product(&before, &mobile_declared::transform_product_id(&session)).unwrap()["metadata"].is_null());

        publisher.align(100, &to_scene(1.))?;
        // The same measurement again is not a new observation.
        publisher.align(100, &to_scene(9.))?;
        publisher.capture(150, &[[0., 0., -1.], [0.1, 0., -1.]], Some(&[[10, 20, 30], [40, 50, 60]]))?;
        publisher.capture(150, &[[9., 9., 9.]], None)?;
        // Endpoints that are not this session into this scene are refused.
        let mut elsewhere = to_scene(1.);
        elsewhere.to_frame_id = "local-scene:nacs:portal-RG3E3LUITXC".into();
        assert!(publisher.align(200, &elsewhere).is_err());

        let catalog = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(catalog["catalog_status"], "ready");
        // The question another peer asks: from which frame, into which, measured on what clock.
        let alignment = product(&catalog, &mobile_declared::transform_product_id(&session)).expect("the transform is exported");
        assert_eq!(alignment["state"]["Buffer"]["entries"], 1);
        let contract = &alignment["metadata"];
        // The same contract as every other transform this peer knows, its pose included.
        assert_eq!(contract["schema"], spatial_home::compact_pose::METADATA);
        assert_eq!(contract["value"]["from_frame"]["peer_id"], phone);
        assert_eq!(contract["value"]["from_frame"]["id"], format!("phone-ar:{session}"));
        // The destination is Home's own frame, by Home's own definition of it.
        let scene = spatial_home::conventions::registry(&home, &scene_frame)?;
        assert_eq!(contract["value"]["to_frame"]["peer_id"], home);
        assert_eq!(contract["value"]["to_frame"]["id"], scene_frame);
        assert_eq!(contract["value"]["to_frame"]["hash"], scene.hash());
        assert_eq!(contract["value"]["clock"]["id"], "arkit-capture");
        assert_eq!(contract["value"]["session_id"], session);
        assert_eq!(contract["value"]["lifetime"], "held", "it stands after the Portal leaves view");

        let points = product(&catalog, &mobile_declared::points_product_id(&session)).expect("depth points are exported");
        assert_eq!(points["state"]["Buffer"]["entries"], 1);
        let contract = &points["metadata"];
        assert_eq!(contract["schema"], mobile_declared::POINTS_METADATA);
        assert_eq!(contract["value"]["frame"]["id"], format!("phone-camera:{session}"));
        // No Product is named: a reader follows the frame.
        assert_eq!(contract["value"].as_object().unwrap().len(), 3);
        // Each Output is in the frame its observations are in, on the capture clock.
        let output = |component: &str, slot: &str| {
            catalog["catalog"]["components"].as_array().unwrap().iter()
                .find(|c| c["manifest"]["component_id"] == component).expect("component is in the catalog")
                ["current_outputs"][slot]["manifest"].clone()
        };
        let transform = output("frame-transforms", "transform");
        assert_eq!(transform["spatial_frame_id"], scene_frame, "a transform's Output is in its destination frame");
        assert_eq!(transform["payload"]["schema"], spatial_home::compact_pose::SCHEMA);
        // The reader's own check of the profile passes on what it fetched.
        let fetched: spatial_home::compact_pose::Contract = serde_json::from_value(alignment["metadata"]["value"].clone())?;
        fetched.validate_output(&serde_json::from_value(transform)?)?;
        assert_eq!(output("depth-points", "points")["spatial_frame_id"], format!("phone-camera:{session}"));
        assert_eq!(output("depth-points", "points")["clock"]["id"], "arkit-capture");

        // Losing alignment is said once, and a new one follows it.
        publisher.lose()?;
        publisher.lose()?;
        publisher.align(300, &to_scene(2.))?;
        let later = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(product(&later, &mobile_declared::transform_product_id(&session)).unwrap()["state"]["Buffer"]["entries"], 3);

        publisher.close()?;
        let closed = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert!(product(&closed, &mobile_declared::transform_product_id(&session)).is_none());
        assert!(product(&closed, &mobile_declared::points_product_id(&session)).is_none());
        products.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(pair.shutdown().await)
}
