mod support;
use anyhow::Result;
use spatial_home::{
    device_catalogs::{Catalogs, fetch},
    mobile_camera::Camera,
    mobile_products::MobileProducts,
    sensor_preview::{self, Request},
};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

async fn preview(tx: &mpsc::Sender<Request>, sensor: Option<String>) -> Result<serde_json::Value> {
    let (reply, answer) = oneshot::channel();
    tx.send(Request {
        scene: false,
        sensor,
        ar: false,
        pick: false,
        reply,
    })
    .await?;
    answer.await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_camera_shares_sdk_catalog_and_home_consumes_only_on_demand() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let products = MobileProducts::mount(&pair.b)?;
    let local_map = spatial_home::mobile_local::Publisher::new(&products)?;
    let mut camera = Camera::new(&products)?;
    let routes = pair
        .b
        .listen_addresses()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let catalog = fetch(&pair.a, pair.b.peer_id(), &routes).await;
    assert_eq!(catalog["catalog_status"], "ready");
    assert!(catalog["catalog"]["components"].as_array().unwrap().len() >= 2);
    let catalogs = Catalogs::from([(pair.b.peer_id().to_string(), catalog)]);
    assert_eq!(sensor_preview::camera_sensors(&catalogs).len(), 1);
    assert!(
        !camera.demanded(),
        "Catalog discovery must not start capture"
    );
    let (_catalog_tx, catalog_rx) = watch::channel(catalogs);
    let (tx, rx) = mpsc::channel(4);
    let (_live_tx, live) = watch::channel(spatial_home::scene_mirror::RemoteScene::default());
    let frames = spatial_home::ar_preview::Frames::default();
    let stop = CancellationToken::new();
    let serve = sensor_preview::serve(&pair.a, catalog_rx, rx, live, &frames, &stop);
    let exercise = async {
        let listing = preview(&tx, None).await?;
        let id = listing["sensors"][0]["id"].as_str().unwrap().to_owned();
        assert!(!camera.demanded());
        let _ = preview(&tx, Some(id.clone())).await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while !camera.demanded() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(
            &image::RgbImage::from_pixel(640, 480, image::Rgb([50, 100, 150])),
        )?;
        camera.publish(100, jpeg.clone())?;
        assert!(camera.publish(100, jpeg).is_err());
        assert!(camera.publish(101, vec![0; 10]).is_err());
        let frame = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(frame) = preview(&tx, Some(id.clone())).await {
                    break frame;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        assert_eq!(frame["width"], 640);
        assert_eq!(frame["height"], 480);
        assert_eq!(frame["capture_timestamp_ns"], 100);
        tokio::time::timeout(Duration::from_secs(5), async {
            while camera.demanded() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await?;
        Ok::<_, anyhow::Error>(())
    };
    let (_, result) = tokio::join!(serve, async {
        let r = exercise.await;
        stop.cancel();
        r
    });
    camera.close()?;
    local_map.close().await?;
    products.close().await?;
    pair.shutdown().await?;
    result
}
