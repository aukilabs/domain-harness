mod support;
use anyhow::Result;
use spatial_home::{
    device_catalogs::fetch,
    mobile_odometry::{COMPONENT, Publisher, product_id},
    mobile_pose::Sample,
    mobile_products::MobileProducts,
};

fn sample(session: &str, timestamp_ns: u64, x: f64) -> Sample {
    Sample {
        camera_configuration_id: None,
        session: session.into(),
        timestamp_ns,
        pose: auki_scenegraph::RigidTransform {
            from_frame_id: format!("phone-camera:{session}"),
            to_frame_id: format!("phone-ar:{session}"),
            translation: [x, 1.5, 0.],
            // Within Home's tolerance, but not exactly unit length.
            rotation_wxyz: [1.0004, 0., 0., 0.],
        },
    }
}

fn product<'a>(catalog: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
    catalog["catalog"]["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["manifest"]["product_id"] == id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_pose_is_declared_with_frames_and_follows_the_ar_session() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let phone = pair.b.peer_id().to_string();
        let home = pair.a.peer_id().to_string();
        let products = MobileProducts::mount(&pair.b)?;
        let routes: Vec<_> = pair
            .b
            .listen_addresses()
            .iter()
            .map(ToString::to_string)
            .collect();
        let first = uuid::Uuid::new_v4().to_string();
        let mut publisher = Publisher::new(&products, &home, &sample(&first, 10, 0.))?;
        publisher.publish(&home, &sample(&first, 10, 0.))?;
        publisher.publish(&home, &sample(&first, 20, 0.1))?;
        // A repeated or older capture time is skipped, not an error.
        publisher.publish(&home, &sample(&first, 20, 9.))?;
        publisher.publish(&home, &sample(&first, 15, 9.))?;

        let catalog = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(catalog["catalog_status"], "ready");
        let component = catalog["catalog"]["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["manifest"]["component_id"] == COMPONENT)
            .expect("phone-pose component is in the catalog");
        let output = &component["current_outputs"]["poses"]["manifest"];
        assert_eq!(output["payload"]["schema"], spatial_home::compact_pose::SCHEMA);
        assert_eq!(output["payload"]["modality"], "pose");
        assert_eq!(output["spatial_frame_id"], format!("phone-ar:{first}"));
        let entry = product(&catalog, &product_id(&first)).expect("pose product is exported");
        assert_eq!(entry["state"]["Buffer"]["entries"], 2);
        let metadata = &entry["metadata"];
        assert_eq!(metadata["schema"], spatial_home::compact_pose::METADATA);
        assert_eq!(metadata["value"]["session_id"], first);
        assert_eq!(metadata["value"]["from_frame"]["peer_id"], phone);
        assert_eq!(
            metadata["value"]["from_frame"]["id"],
            format!("phone-camera:{first}")
        );
        assert_eq!(metadata["value"]["to_frame"]["peer_id"], phone);
        assert_eq!(metadata["value"]["clock"]["id"], "arkit-capture");
        let contract:spatial_home::compact_pose::Contract=serde_json::from_value(metadata["value"].clone())?;
        let manifest:auki_components::OutputManifest=serde_json::from_value(output.clone())?;
        contract.validate_output(&manifest)?;
        let registries=auki_protocols::registry::RegistryClient::new(pair.a.protocols());
        let route:auki_sdk::Multiaddr=routes[0].parse()?;
        let from=registries.fetch_frame_exact(pair.b.peer_id(),route.clone(),&contract.from_frame.id,&contract.from_frame.hash).await?;
        let to=registries.fetch_frame_exact(pair.b.peer_id(),route.clone(),&contract.to_frame.id,&contract.to_frame.hash).await?;
        let clock=registries.fetch_clock_exact(pair.b.peer_id(),route.clone(),&contract.clock.id,&contract.clock.hash).await?;
        assert_eq!(from.frame_id,format!("phone-camera:{first}"));
        assert_eq!(to.frame_id,format!("phone-ar:{first}"));
        assert_eq!(clock.hash(),contract.clock.hash);
        let client=auki_component_protocol::ComponentProtocolClient::new(pair.a.protocols());
        let observations=client.observations_exact::<spatial_home::compact_pose::Update>(pair.b.peer_id(),route,
            auki_component_protocol::ObservationRequest{
                product:auki_components::ProductReference{peer_id:phone.clone(),product_id:product_id(&first),manifest_hash:entry["manifest_hash"].as_str().unwrap().into()},
                selection:auki_component_protocol::ObservationSelection::LatestExisting}).await?;
        let value=serde_json::to_value(&observations.observations[0].payload)?;
        assert!(value.get("contract").is_none());
        assert_eq!(value["tracking"]["status"],"valid");
        assert_eq!(value["tracking"]["pose"]["translation"][0],0.1);

        // Losing the pose is said once and retained as an observation.
        publisher.lost()?;
        publisher.lost()?;
        let catalog = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(
            product(&catalog, &product_id(&first)).unwrap()["state"]["Buffer"]["entries"],
            3
        );

        // An AR reset is a new product on the same component; the old one goes.
        let second = uuid::Uuid::new_v4().to_string();
        publisher.publish(&home, &sample(&second, 5, 0.))?;
        let catalog = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert!(product(&catalog, &product_id(&first)).is_none());
        let entry = product(&catalog, &product_id(&second)).expect("new session product");
        assert_eq!(entry["state"]["Buffer"]["entries"], 1);
        assert_eq!(entry["metadata"]["value"]["session_id"], second);
        assert_eq!(
            catalog["catalog"]["components"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["manifest"]["component_id"] == COMPONENT)
                .count(),
            1
        );

        publisher.close()?;
        let catalog = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert!(
            catalog["catalog_status"] != "ready"
                || product(&catalog, &product_id(&second)).is_none()
        );
        products.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(pair.shutdown().await)
}
