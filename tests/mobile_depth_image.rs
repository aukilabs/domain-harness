mod support;
use anyhow::Result;
use base64::Engine;
use spatial_home::{
    device_catalogs::fetch,
    mobile_depth_image::{COMPONENT, DepthImage, PRODUCT, Publisher},
    mobile_products::MobileProducts,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_depth_image_is_declared_and_read_back_exactly() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let products = MobileProducts::mount(&pair.b)?;
        let routes: Vec<_> = pair
            .b
            .listen_addresses()
            .iter()
            .map(ToString::to_string)
            .collect();
        let session = uuid::Uuid::new_v4().to_string();
        let mut publisher = Publisher::new(&products, &session)?;
        let (width, height) = (256u32, 192u32);
        let depth: Vec<u16> = (0..width * height).map(|i| (i % 5000) as u16).collect();
        let confidence: Vec<u8> = (0..width * height).map(|i| (i % 3) as u8).collect();
        let intrinsics = [212., 212.5, 128., 96.];
        let image = DepthImage::new(&session, 100, (width, height), &depth, &confidence, intrinsics, 42)?;
        // Wrong pixel counts and bad intrinsics never become an observation.
        assert!(DepthImage::new(&session, 100, (width, height), &depth[1..], &confidence, intrinsics, 0).is_err());
        assert!(DepthImage::new(&session, 100, (width, height), &depth, &confidence, [0., 1., 1., 1.], 0).is_err());
        assert!(DepthImage::new("not-a-session", 100, (width, height), &depth, &confidence, intrinsics, 0).is_err());
        publisher.publish(image.clone())?;
        // The same capture again is skipped, not an error.
        publisher.publish(image.clone())?;

        let catalog = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(catalog["catalog_status"], "ready");
        let component = catalog["catalog"]["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["manifest"]["component_id"] == COMPONENT)
            .expect("depth image component");
        let output = &component["current_outputs"]["frames"]["manifest"];
        // The same camera frame and capture clock as the picture, depth and pose.
        assert_eq!(output["spatial_frame_id"], format!("phone-camera:{session}"));
        assert_eq!(output["clock"]["id"], "arkit-capture");
        assert_eq!(output["payload"]["unit"], "millimeters");
        let entry = catalog["catalog"]["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["manifest"]["product_id"] == PRODUCT)
            .expect("depth image product");
        assert_eq!(entry["state"]["Buffer"]["entries"], 1);
        let metadata = &entry["metadata"]["value"];
        assert_eq!(metadata["width"], 256);
        assert_eq!(metadata["camera_product_id"], "phone-camera");
        assert_eq!(metadata["pose_product_id"], format!("phone-pose:{session}"));
        assert_eq!(metadata["alignment_timestamp_ns"], 42);

        // A reader gets the same millimetres and confidence back.
        let client = auki_component_protocol::ComponentProtocolClient::new(pair.a.protocols());
        let reference = auki_components::ProductReference {
            peer_id: pair.b.peer_id().to_string(),
            product_id: PRODUCT.into(),
            manifest_hash: entry["manifest_hash"].as_str().unwrap().into(),
        };
        let read = client
            .observations_exact::<DepthImage>(
                pair.b.peer_id(),
                routes[0].parse()?,
                auki_component_protocol::ObservationRequest {
                    product: reference,
                    selection: auki_component_protocol::ObservationSelection::LatestExisting,
                },
            )
            .await?;
        let got = &read.observations[0];
        assert_eq!(got.timestamp_ns, 100);
        assert_eq!(*got.payload, image);
        let engine = base64::engine::general_purpose::STANDARD;
        let bytes = engine.decode(&got.payload.depth_mm)?;
        let back: Vec<u16> = bytes.chunks(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(back, depth);
        assert_eq!(engine.decode(&got.payload.confidence)?, confidence);

        // A new AR session is a new Output on the same Component.
        let next = uuid::Uuid::new_v4().to_string();
        publisher.publish(DepthImage::new(&next, 5, (width, height), &depth, &confidence, intrinsics, 0)?)?;
        let catalog = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        let entry = catalog["catalog"]["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["manifest"]["product_id"] == PRODUCT)
            .unwrap();
        assert_eq!(entry["metadata"]["value"]["camera_frame"], format!("phone-camera:{next}"));
        assert!(entry["metadata"]["value"]["pose_product_id"].is_null());
        publisher.close()?;
        products.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(pair.shutdown().await)
}
