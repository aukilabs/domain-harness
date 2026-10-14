mod support;
use anyhow::Result;
use spatial_home::{mobile_camera::{Camera, Geometry, PRODUCT}, mobile_products::MobileProducts, registry_ar};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn annotator_camera_is_consumed_by_generic_registry_reader() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let products = MobileProducts::mount(&pair.b)?;
    let mut camera = Camera::new(&products)?;
    let session = uuid::Uuid::new_v4().to_string();
    let geometry = Geometry {session, fx:480.,fy:481.,cx:320.,cy:240.,rotation_deg:0,alignment_timestamp_ns:0};
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(
        &image::RgbImage::from_pixel(640,480,image::Rgb([50,100,150])))?;
    let routes = pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
    let mut identities = Vec::new();
    for (timestamp,fx) in [(100,480.),(101,480.),(102,490.)] {
        camera.publish_calibrated(timestamp,jpeg.clone(),Geometry {fx,..geometry.clone()})?;
        let mut row = spatial_home::device_catalogs::fetch(&pair.a,pair.b.peer_id(),&routes).await;
        row["registries"] = spatial_home::product_registry::resolve(&pair.a,pair.b.peer_id(),&row,None,&Default::default()).await;
        let catalog: auki_components::CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
        let p = catalog.products.iter().find(|p|p.manifest.product_id==PRODUCT).unwrap();
        assert!(registry_ar::supported(p,&catalog,&row["registries"]),"{row}");
        let output = catalog.components.iter().flat_map(|c|c.current_outputs.values())
            .find(|o|o.manifest.reference()==p.manifest.producer).unwrap();
        let (contract,calibration) = registry_ar::camera(p.metadata.as_ref().unwrap(),&output.manifest,&row["registries"])?;
        assert_eq!(contract.session_id,geometry.session);
        assert_eq!(contract.clock.id,"arkit-capture");
        assert_eq!(calibration["K"][0],fx);
        identities.push(p.manifest.reference());
    }
    assert_eq!(identities[0],identities[1]);
    assert_ne!(identities[1],identities[2]);
    assert!(camera.publish_calibrated(103,jpeg,Geometry {rotation_deg:180,..geometry}).is_err());
    camera.close()?;
    products.close().await?;
    pair.shutdown().await?;
    Ok(())
}
