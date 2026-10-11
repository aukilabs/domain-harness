mod support;
use anyhow::Result;
use spatial_home::{
    device_catalogs::{fetch, Catalogs},
    sensor_preview::{self, Request},
    simulated_camera::Camera,
};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

async fn request(tx: &mpsc::Sender<Request>, sensor: Option<String>) -> Result<serde_json::Value> {
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
async fn discovered_camera_uses_real_catalog_and_subscription_demand() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let mut camera = Camera::mount(&pair.b)?;
    let routes = pair
        .b
        .listen_addresses()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let row = fetch(&pair.a, pair.b.peer_id(), &routes).await;
    assert_eq!(row["catalog_status"], "ready");
    let mut catalogs = Catalogs::new();
    catalogs.insert(pair.b.peer_id().to_string(), row.clone());
    assert_eq!(sensor_preview::camera_sensors(&catalogs).len(), 1);
    assert!(!camera.demanded(), "Catalog reads must not activate camera");
    let mut spoofed = row.clone();
    spoofed["catalog"]["products"][0]["manifest"]["peer_id"] = "wrong-owner".into();
    let invalid = Catalogs::from([(pair.b.peer_id().to_string(), spoofed)]);
    assert!(sensor_preview::camera_sensors(&invalid).is_empty());
    let (catalog_tx, catalog_rx) = watch::channel(catalogs);
    let (tx, rx) = mpsc::channel(4);
    let (_, live) = watch::channel(spatial_home::scene_mirror::RemoteScene::default());
    let frames = spatial_home::ar_preview::Frames::default();
    let stop = CancellationToken::new();
    let serve = sensor_preview::serve(&pair.a, catalog_rx, rx, live, &frames, &stop);
    let exercise = async {
        let listing = request(&tx, None).await?;
        let id = listing["sensors"][0]["id"].as_str().unwrap().to_owned();
        assert!(!camera.demanded());
        let _ = request(&tx, Some(id.clone())).await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while !camera.demanded() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        let mut jpeg = Vec::new();
        let image = image::RgbImage::from_pixel(640, 480, image::Rgb([50, 100, 150]));
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(&image)?;
        camera.publish(1, jpeg.clone())?;
        assert!(camera.publish(1, jpeg.clone()).is_err());
        assert!(camera.publish(2, vec![0; 10]).is_err());
        let frame = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(frame) = request(&tx, Some(id.clone())).await {
                    break frame;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        assert_eq!(frame["width"], 640);
        assert_eq!(frame["capture_timestamp_ns"], 1);
        assert_eq!(camera.endpoint.active_subscriptions(), 1);
        // A second browser request reuses the same subscription.
        request(&tx, Some(id.clone())).await?;
        assert_eq!(camera.endpoint.active_subscriptions(), 1);
        // A delayed catalog refresh must not tear down the live subscription.
        let mut cached = row.clone();
        cached["catalog_cached"] = true.into();
        cached["catalog_refresh_status"] = "unavailable".into();
        catalog_tx.send_replace(Catalogs::from([(pair.b.peer_id().to_string(), cached)]));
        tokio::time::sleep(Duration::from_millis(150)).await;
        request(&tx, Some(id.clone())).await?;
        assert_eq!(camera.endpoint.active_subscriptions(), 1);
        // Cached metadata must not turn an old image into a live frame.
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = request(&tx, Some(id.clone())).await;
        }
        assert!(request(&tx, Some(id.clone()))
            .await
            .unwrap_err()
            .to_string()
            .contains("stale"));
        tokio::time::timeout(Duration::from_secs(5), async {
            while camera.demanded() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await?;
        catalog_tx.send_replace(Catalogs::new());
        assert!(request(&tx, None).await?["sensors"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(request(&tx, Some(id)).await.is_err());
        Ok::<_, anyhow::Error>(())
    };
    let wrapped = async {
        let result = exercise.await;
        stop.cancel();
        result
    };
    let (_, result) = tokio::join!(serve, wrapped);
    camera.close().await?;
    pair.shutdown().await?;
    result
}
