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
        home_frame: None, debug_pose: false,
        scene: false,
        sensor,
        ar: false,
        pick: false,
        follow: false,
        capture: false,
        expected_manifest: None,
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
    let dir=tempfile::tempdir()?;
    let homes=spatial_home::local_scenes::LocalScenes::load(dir.path())?;
    let serve = sensor_preview::serve(&pair.a, catalog_rx, rx, live, &frames, &homes, &stop);
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
        tokio::time::timeout(Duration::from_secs(35), async {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_camera_joins_the_ar_session_and_declares_its_calibration() -> Result<()> {
    use spatial_home::mobile_camera::Geometry;
    let pair = support::LocalPair::start().await?;
    let result = async {
        let products = MobileProducts::mount(&pair.b)?;
        let mut camera = Camera::new(&products)?;
        let routes = pair
            .b
            .listen_addresses()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let frame_of = |catalog: &serde_json::Value| {
            catalog["catalog"]["components"][0]["current_outputs"]["frames"]["manifest"].clone()
        };
        let before = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(before["catalog_status"], "ready");
        assert!(
            frame_of(&before)["spatial_frame_id"]
                .as_str()
                .unwrap()
                .starts_with("phone-rgb:")
        );

        // Knowing the AR session is enough to relate the camera to depth and pose.
        let session = uuid::Uuid::new_v4().to_string();
        camera.enter_session(&session, 50)?;
        camera.enter_session(&session, 60)?;
        let entered = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(
            frame_of(&entered)["spatial_frame_id"],
            format!("phone-camera:{session}")
        );
        assert_eq!(frame_of(&entered)["clock"]["id"], "arkit-capture");
        assert_eq!(entered["catalog"]["products"].as_array().unwrap().len(), 1);

        let jpeg = |width, height| -> Result<Vec<u8>> {
            let mut jpeg = Vec::new();
            image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(
                &image::RgbImage::from_pixel(width, height, image::Rgb([50, 100, 150])),
            )?;
            Ok(jpeg)
        };
        let geometry = Geometry { session: session.clone(), fx: 480., fy: 481., cx: 320., cy: 240., rotation_deg: 0, alignment_timestamp_ns: 0 };
        camera.publish_calibrated(100, jpeg(640, 480)?, geometry.clone())?;
        assert!(
            camera
                .publish_calibrated(101, jpeg(640, 480)?, Geometry { fx: f64::NAN, ..geometry.clone() })
                .is_err()
        );
        assert!(
            camera
                .publish_calibrated(101, jpeg(640, 480)?, Geometry { rotation_deg: 45, ..geometry.clone() })
                .is_err()
        );
        let sent = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        // An image that is not turned stays in the camera frame depth and pose use.
        assert_eq!(
            frame_of(&sent)["spatial_frame_id"],
            format!("phone-camera:{session}")
        );
        let metadata = &sent["catalog"]["products"][0]["metadata"];
        assert_eq!(metadata["schema"], "spatial.phone-camera/v1");
        assert_eq!(metadata["value"]["calibration"]["K"][0], 480.);
        assert_eq!(metadata["value"]["calibration"]["K"][4], 481.);
        assert_eq!(metadata["value"]["calibration"]["width"], 640);
        assert_eq!(metadata["value"]["rotation_from_camera_frame_deg"], 0);
        // `pose_product_id` is the name every producer uses, robots included.
        // Not aligned: there is no pose to match this frame with.
        assert!(metadata["value"]["pose_product_id"].is_null());
        assert_eq!(metadata["value"]["depth_image_product_id"], "phone-depth-image");
        assert!(sensor_preview::camera_sensors(&Catalogs::from([(
            pair.b.peer_id().to_string(),
            sent.clone()
        )]))
        .len() == 1);

        // Held upright, the phone sends the image turned: 480 wide, 640 tall,
        // in a frame of its own that says which camera frame it was turned from.
        let upright = Geometry { fx: 481., fy: 480., cx: 240., cy: 320., rotation_deg: 90, ..geometry.clone() };
        assert!(camera.publish_calibrated(200, jpeg(640, 480)?, upright.clone()).is_err());
        camera.publish_calibrated(201, jpeg(480, 640)?, upright.clone())?;
        // Entering the same session again does not turn the camera back.
        camera.enter_session(&session, 202)?;
        camera.publish_calibrated(203, jpeg(480, 640)?, upright)?;
        let turned = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        let output = frame_of(&turned);
        assert_eq!(output["payload"]["width"], 480);
        assert_eq!(output["payload"]["height"], 640);
        assert_eq!(
            output["spatial_frame_id"],
            format!("phone-camera:{session}:turned-90")
        );
        let metadata = &turned["catalog"]["products"][0]["metadata"]["value"];
        assert_eq!(metadata["calibration"]["width"], 480);
        assert_eq!(metadata["calibration"]["height"], 640);
        assert_eq!(metadata["rotation_from_camera_frame_deg"], 90);
        assert_eq!(metadata["camera_frame"], format!("phone-camera:{session}"));

        // A bigger picture is a new Output too, and an aligned frame names the
        // pose Product that holds its capture-time pose.
        let big = Geometry { fx: 721., fy: 720., cx: 360., cy: 480., rotation_deg: 90, alignment_timestamp_ns: 77, ..geometry.clone() };
        camera.publish_calibrated(300, jpeg(720, 960)?, big.clone())?;
        // Not the camera's 4:3 shape, too large, or on its side without a quarter turn.
        assert!(camera.publish_calibrated(301, jpeg(720, 900)?, big.clone()).is_err());
        assert!(camera.publish_calibrated(301, jpeg(1200, 1600)?, big.clone()).is_err());
        assert!(camera.publish_calibrated(301, jpeg(960, 720)?, big).is_err());
        let bigger = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(frame_of(&bigger)["payload"]["height"], 960);
        let metadata = &bigger["catalog"]["products"][0]["metadata"]["value"];
        assert_eq!(metadata["calibration"]["width"], 720);
        assert_eq!(metadata["pose_product_id"], format!("phone-pose:{session}"));
        assert_eq!(metadata["alignment_timestamp_ns"], 77);

        // A new AR session moves the camera again.
        let next = uuid::Uuid::new_v4().to_string();
        camera.publish_calibrated(5, jpeg(640, 480)?, Geometry { session: next.clone(), ..geometry })?;
        let moved = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        assert_eq!(
            frame_of(&moved)["spatial_frame_id"],
            format!("phone-camera:{next}")
        );
        camera.close()?;
        products.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(pair.shutdown().await)
}

/// Home draws its scene over a phone's camera through the same path it uses
/// for a robot: exact frame references resolve its pose, image and pose share a
/// frame, a clock and a capture time, and Home reads the camera in the axes
/// it declares.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn home_draws_over_a_phone_camera_without_a_phone_adapter() -> Result<()> {
    use spatial_home::mobile_camera::Geometry;
    let pair = support::LocalPair::start().await?;
    let products = MobileProducts::mount(&pair.b)?;
    let mut camera = Camera::new(&products)?;
    let dir = tempfile::tempdir()?;
    let homes = support::seeded_local_scenes(dir.path())?;
    let home_frame = homes.snapshots["humanoid-hub-conference"].scenegraph.map.frame.id.clone();
    let home_peer = pair.a.peer_id().to_string();
    let session = uuid::Uuid::new_v4().to_string();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode_image(&image::RgbImage::from_pixel(640, 480, image::Rgb([50, 100, 150])))?;
    // One capture: the picture as the sensor took it, and the camera's pose at that instant.
    let geometry = Geometry { session: session.clone(), fx: 480., fy: 481., cx: 320., cy: 240., rotation_deg: 0, alignment_timestamp_ns: 77 };
    camera.publish_calibrated(1000, jpeg, geometry)?;
    let sample = spatial_home::mobile_pose::Sample {
        session: session.clone(),
        timestamp_ns: 1000,
        pose: auki_scenegraph::RigidTransform {
            from_frame_id: format!("phone-camera:{session}"),
            to_frame_id: format!("phone-ar:{session}"),
            translation: [1., 1.5, 2.],
            rotation_wxyz: [1., 0., 0., 0.],
        },
        camera_configuration_id: None,
    };
    let mut pose = spatial_home::mobile_odometry::Publisher::new(&products, &home_peer, &sample)?;
    pose.publish(&home_peer, &sample)?;

    let mut declared = spatial_home::mobile_declared::Publisher::new(&products, &session, &home_peer, &home_frame)?;
    declared.align(77, &auki_scenegraph::RigidTransform {
        from_frame_id: format!("phone-ar:{session}"), to_frame_id: home_frame.clone(),
        translation:[0.;3], rotation_wxyz:[1.,0.,0.,0.],
    })?;
    let definitions=spatial_home::product_registry::Definitions::default();
    spatial_home::product_registry::frame(&definitions,&spatial_home::conventions::registry(&home_peer,&home_frame)?)?;

    let routes = pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
    let mut row=fetch(&pair.a, pair.b.peer_id(), &routes).await;
    row["registries"]=spatial_home::product_registry::resolve(&pair.a,pair.b.peer_id(),&row,None,&definitions).await;
    let catalogs = Catalogs::from([(pair.b.peer_id().to_string(), row)]);
    let (_catalog_tx, catalog_rx) = watch::channel(catalogs);
    let (tx, rx) = mpsc::channel(4);
    let (_live_tx, live) = watch::channel(spatial_home::scene_mirror::RemoteScene::default());
    let frames = spatial_home::ar_preview::Frames::default();
    let stop = CancellationToken::new();
    let serve = sensor_preview::serve(&pair.a, catalog_rx, rx, live, &frames, &homes, &stop);
    let exercise = async {
        let listing = preview(&tx, None).await?;
        let listed = listing["sensors"].as_array().unwrap().iter().find(|s| s["label"] == "phone-camera").unwrap();
        assert_eq!(listed["ar_available"], true, "the camera's pose is found by exact frame and clock");
        let id = listed["id"].as_str().unwrap().to_owned();
        let mut last = String::new();
        let drawn = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let (reply, answer) = oneshot::channel();
                tx.send(Request { home_frame: None, debug_pose: false, scene: false, sensor: Some(id.clone()), ar: true, pick: false, follow: false,
                    capture: false, expected_manifest: None, reply }).await?;
                match answer.await? {
                    Ok(frame) => break Ok::<_, anyhow::Error>(frame),
                    Err(error) => last = error.to_string(),
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("Home never drew over the phone camera: {last}"))??;
        assert_eq!(drawn["capture_timestamp_ns"], 1000);
        assert_eq!(drawn["ar"]["status"], "ready");
        assert_eq!(drawn["ar"]["frame"], home_frame);
        let seen = &drawn["picking"]["camera_to_map"];
        assert_eq!(seen["translation"], serde_json::json!([1., 1.5, 2.]), "the camera is where the phone said");
        // The phone's camera looks along -Z with Y up. Read as an optical
        // camera, that is half a turn about X.
        let q: Vec<f64> = serde_json::from_value(seen["rotation_xyzw"].clone())?;
        assert!((q[0].abs() - 1.).abs() < 1e-9 && q[1].abs() + q[2].abs() + q[3].abs() < 1e-9, "{q:?}");
        Ok::<_, anyhow::Error>(())
    };
    let (_, result) = tokio::join!(serve, async {
        let r = exercise.await;
        stop.cancel();
        r
    });
    pose.close()?;
    camera.close()?;
    products.close().await?;
    pair.shutdown().await?;
    result
}
