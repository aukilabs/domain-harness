mod support;
use anyhow::Result;
use auki_component_protocol::{ComponentProtocolClient, ObservationStart, RemoteObservationEvent};
use auki_components::{BufferLimits, ProductReference, VideoFrame};
use spatial_home::{
    device_catalogs::fetch,
    mobile_products::MobileProducts,
    mobile_screen::{PRODUCT, Screen},
};
use std::time::Duration;

fn jpeg((width, height): (u32, u32)) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb([200, 40, 40]));
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 60).encode_image(&image).unwrap();
    bytes
}

/// The screen is offered to Home as a camera, and captured only while
/// someone is subscribed: opening it is the request, leaving ends it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_screen_is_sent_only_while_someone_watches() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let products = MobileProducts::mount(&pair.b)?;
        let routes: Vec<_> = pair.b.listen_addresses().iter().map(ToString::to_string).collect();
        assert!(Screen::new(&products, (296, 720)).is_err());
        assert!(Screen::new(&products, (100, 640)).is_err());
        let size = (296, 640);
        let mut screen = Screen::new(&products, size)?;
        assert!(!screen.demanded());

        // Home lists it with its other cameras, as a plain feed with no pose.
        let catalog = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(catalog["catalog_status"], "ready");
        let owner = pair.b.peer_id().to_string();
        let catalogs = std::collections::BTreeMap::from([(owner.clone(), catalog.clone())]);
        let sensors = spatial_home::sensor_preview::camera_choices(&catalogs);
        assert!(sensors.iter().any(|s| s["product_id"] == PRODUCT), "{sensors:?}");
        let entry = catalog["catalog"]["products"].as_array().unwrap().iter()
            .find(|p| p["manifest"]["product_id"] == PRODUCT).unwrap();

        let client = ComponentProtocolClient::new(pair.a.protocols());
        let mut subscription = client
            .subscribe_product_exact_with_decoder(
                pair.b.peer_id(),
                routes[0].parse()?,
                ProductReference {
                    peer_id: owner,
                    product_id: PRODUCT.into(),
                    manifest_hash: entry["manifest_hash"].as_str().unwrap().into(),
                },
                ObservationStart::NewOnly,
                BufferLimits::entries(1),
                |f: &VideoFrame| f.bytes.len(),
                // Screen pictures go out in the binary camera format, never as JSON.
                spatial_home::camera_frame::decoder(spatial_home::camera_frame::SCHEMA)?,
            )
            .await?;
        assert!(screen.demanded());
        // A picture of another size is refused; the right size arrives.
        assert!(screen.publish(1, jpeg((320, 640))).is_err());
        screen.publish(2, jpeg(size))?;
        assert!(screen.publish(2, jpeg(size)).is_err(), "time must advance");
        let Some(RemoteObservationEvent::Observation(frame)) =
            tokio::time::timeout(Duration::from_secs(5), subscription.next()).await??
        else {
            panic!("expected a screen frame")
        };
        assert_eq!((frame.payload.width, frame.payload.height, frame.payload.encoding.as_str()), (296, 640, "jpeg"));
        assert_eq!(frame.timestamp_ns, 2);
        // Once a frame exists, the Catalog says it is already upright.
        let catalog = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        let entry = catalog["catalog"]["products"].as_array().unwrap().iter()
            .find(|p| p["manifest"]["product_id"] == PRODUCT).unwrap();
        assert_eq!(entry["metadata"]["value"]["presentation"]["rotation_clockwise_deg"], 0);

        subscription.close().await?;
        tokio::time::timeout(Duration::from_secs(3), async {
            while screen.demanded() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        screen.close()?;
        products.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(pair.shutdown().await)
}
