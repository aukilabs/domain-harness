mod support;
use anyhow::Result;
use spatial_home::{capture_time, mobile_products::MobileProducts};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_catalog_exports_sdk_session_clock_and_utc_transform() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let products = MobileProducts::mount(&pair.b)?;
    let routes = pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
    let row = spatial_home::device_catalogs::fetch(&pair.a, pair.b.peer_id(), &routes).await;
    let catalog: auki_components::CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
    let product = catalog.products.iter().find(|p| p.metadata.as_ref().is_some_and(|m| m.schema == capture_time::METADATA)).unwrap();
    let contract: capture_time::Contract = serde_json::from_value(product.metadata.as_ref().unwrap().value.clone())?;
    let output = catalog.components.iter().flat_map(|c| c.current_outputs.values())
        .find(|o| o.manifest.reference() == product.manifest.producer).unwrap();
    contract.validate(&output.manifest)?;
    assert_eq!(contract.from_clock, products.timing.clock.registry_entry());
    assert!(contract.utc_accuracy_ns.is_none(), "A local clock read does not prove UTC synchronization");
    let definitions = products.definitions.lock().unwrap();
    assert!(definitions.iter().any(|d| d.hash == contract.from_clock.hash()));
    assert!(definitions.iter().any(|d| d.hash == contract.to_clock.hash()));
    drop(definitions);
    products.close().await?;
    pair.shutdown().await?;
    Ok(())
}
