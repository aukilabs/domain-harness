mod support;
use anyhow::Result;
use spatial_home::{
    device_catalogs::{Catalogs, device_rows, fetch},
    mobile_local::Publisher,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalogs_are_real_refreshable_and_not_retained_as_live_peers() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let publisher = Publisher::new(&products)?;
        let routes: Vec<_> = pair
            .b
            .listen_addresses()
            .iter()
            .map(ToString::to_string)
            .collect();
        let catalog = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(catalog["catalog_status"], "ready");
        assert!(
            catalog["catalog"]["components"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["manifest"]["component_id"] == "phone-local-mapper")
        );
        assert!(
            catalog["catalog"]["products"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["manifest"]["product_id"] == "phone-local-voxels")
        );
        let unchanged = spatial_home::device_catalogs::fetch_revision(&pair.a, pair.b.peer_id(), &routes, Some(&catalog)).await;
        assert_eq!(unchanged["catalog_status"], "ready");
        assert_eq!(unchanged["catalog"], catalog["catalog"]);
        let mut cache = Catalogs::new();
        cache.insert(pair.b.peer_id().to_string(), catalog);
        // The phone owns its map even without portal alignment or a Home push.
        let session = uuid::Uuid::new_v4().to_string();
        publisher.publish(spatial_home::mobile_local::Snapshot {
            session: session.clone(), timestamp_ns: 1, cells: vec![[1, 2, 3]],
        })?;
        let changed = spatial_home::device_catalogs::fetch_revision(&pair.a, pair.b.peer_id(), &routes, cache.get(&pair.b.peer_id().to_string())).await;
        assert!(changed["catalog"]["revision"].as_u64().unwrap() > unchanged["catalog"]["revision"].as_u64().unwrap());
        let scenes = spatial_home::sensor_preview::scene_products(&cache);
        assert_eq!(scenes.len(), 1);
        assert!(spatial_home::mobile_local::render(&pair.b.peer_id().to_string()).is_err());
        let client = auki_component_protocol::ComponentProtocolClient::new(pair.a.protocols());
        let view = spatial_home::sensor_preview::read_scene(&client, &scenes[0]).await?;
        assert_eq!(view["authority_peer_id"], pair.b.peer_id().to_string());
        assert_eq!(view["product_id"], "phone-local-voxels");
        assert_eq!(view["source_observation"]["clock"], serde_json::json!(
            spatial_home::model::clock_ref(&products.timing.clock.registry_entry())));
        assert_eq!(view["source_observation"]["timestamp_ns"], "1");
        assert_eq!(view["source_observation"]["product"]["product_id"], "phone-local-voxels");
        assert!(view["frame"].as_str().unwrap().ends_with(&session));
        assert_eq!(view["voxels"].as_array().unwrap().len(), 1);
        let next_session = uuid::Uuid::new_v4().to_string();
        publisher.publish(spatial_home::mobile_local::Snapshot {
            session: next_session.clone(), timestamp_ns: 2, cells: vec![],
        })?;
        let view = spatial_home::sensor_preview::read_scene(&client, &scenes[0]).await?;
        assert!(view["frame"].as_str().unwrap().ends_with(&next_session));
        assert!(view["voxels"].as_array().unwrap().is_empty());
        cache.insert(
            "not-a-connected-peer".into(),
            serde_json::json!({"catalog_status":"ready"}),
        );
        let rows = device_rows(&pair.a, &cache);
        assert!(rows.iter().any(
            |r| r["peer_id"] == pair.b.peer_id().to_string() && r["catalog_status"] == "ready"
        ));
        assert!(!rows.iter().any(|r| r["peer_id"] == "not-a-connected-peer"));
        publisher.close().await?;
        products.close().await?;
        let unavailable = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(unavailable["catalog_status"], "unavailable");
        assert!(unavailable.get("catalog").is_none());
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let restored = Publisher::new(&products)?;
        assert_eq!(
            fetch(&pair.a, pair.b.peer_id(), &routes).await["catalog_status"],
            "ready"
        );
        restored.close().await?;
        products.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(pair.shutdown().await)
}
