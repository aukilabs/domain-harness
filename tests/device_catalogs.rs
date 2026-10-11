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
                .any(|c| c["manifest"]["component_id"] == "android-local-mapper")
        );
        assert!(
            catalog["catalog"]["products"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["manifest"]["product_id"] == "phone-local-voxels")
        );
        let mut cache = Catalogs::new();
        cache.insert(pair.b.peer_id().to_string(), catalog);
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
