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

/// What the phone sends reaches a reader through the SDK's binary camera codec, unchanged:
/// the same image bytes, capture times and sequence, in the Output whose frame and clock the
/// camera contract names. A JSON reader cannot read it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn annotator_camera_frames_arrive_binary_and_unchanged() -> Result<()> {
    use auki_component_protocol::{ComponentProtocolClient, ObservationStart, RemoteObservationEvent};
    use auki_components::{BufferLimits, PayloadContract, VideoFrame};
    let pair = support::LocalPair::start().await?;
    let products = MobileProducts::mount(&pair.b)?;
    let mut camera = Camera::new(&products)?;
    let geometry = Geometry {session:uuid::Uuid::new_v4().to_string(), fx:480.,fy:481.,cx:320.,cy:240.,rotation_deg:0,alignment_timestamp_ns:0};
    let image = |shade: u8| -> Result<Vec<u8>> {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(&image::RgbImage::from_pixel(640,480,image::Rgb([shade,100,150])))?;
        Ok(jpeg)
    };
    let sent = [(1_000_000_123u64, image(10)?), (1_100_000_456, image(200)?), (1_200_000_789, image(90)?)];
    for (timestamp, jpeg) in &sent { camera.publish_calibrated(*timestamp, jpeg.clone(), geometry.clone())?; }

    let routes = pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
    let mut row = spatial_home::device_catalogs::fetch(&pair.a,pair.b.peer_id(),&routes).await;
    row["registries"] = spatial_home::product_registry::resolve(&pair.a,pair.b.peer_id(),&row,None,&Default::default()).await;
    let catalog: auki_components::CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
    let product = catalog.products.iter().find(|p|p.manifest.product_id==PRODUCT).unwrap();
    let output = catalog.components.iter().flat_map(|c|c.current_outputs.values())
        .find(|o|o.manifest.reference()==product.manifest.producer).unwrap();
    let PayloadContract::Camera(payload) = &output.manifest.payload else { panic!("camera payload expected") };
    assert_eq!(payload.schema, spatial_home::camera_frame::SCHEMA);
    let (contract, _) = registry_ar::camera(product.metadata.as_ref().unwrap(),&output.manifest,&row["registries"])?;
    contract.validate(&output.manifest)?;

    let client = ComponentProtocolClient::new(pair.a.protocols());
    let peer = pair.b.peer_id();
    let read = |decoder| { let client = client.clone(); let route = routes[0].clone(); let reference = product.manifest.reference();
        async move { client.subscribe_product_exact_with_decoder(peer, route.parse()?, reference,
            ObservationStart::FromSequence { sequence: 0 }, BufferLimits::entries(8), |f: &VideoFrame| f.bytes.len(), decoder).await
            .map_err(anyhow::Error::from) } };
    let mut stream = read(spatial_home::camera_frame::decoder(&payload.schema)?).await?;
    let mut last_sequence = None;
    for (timestamp, jpeg) in &sent {
        let event = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next()).await??.unwrap();
        let RemoteObservationEvent::Observation(received) = event else { panic!("expected a camera observation") };
        assert_eq!(received.timestamp_ns, *timestamp);
        assert_eq!(received.payload.bytes.as_ref(), jpeg.as_slice());
        assert_eq!((received.payload.width, received.payload.height, received.payload.encoding.as_str()), (640, 480, "jpeg"));
        assert_eq!(received.output, output.manifest.reference());
        assert!(last_sequence.is_none_or(|last| received.sequence > last));
        last_sequence = Some(received.sequence);
    }
    // A reader expecting the old JSON frames gets nothing it can decode.
    let legacy = read(spatial_home::camera_frame::decoder(spatial_home::camera_frame::LEGACY_SCHEMA)?).await;
    if let Ok(mut legacy) = legacy {
        let event = tokio::time::timeout(std::time::Duration::from_secs(3), legacy.next()).await?;
        assert!(!matches!(event, Ok(Some(RemoteObservationEvent::Observation(_)))), "a JSON reader decoded a binary frame");
    }
    camera.close()?;
    products.close().await?;
    pair.shutdown().await?;
    Ok(())
}
