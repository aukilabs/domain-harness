use super::*;
use ::auki_sdk::{AukiPeer, Identity};
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ObservationStart, RemoteObservationEvent,
};
#[path = "test_support/mod.rs"]
pub(crate) mod support;

pub(crate) fn image(id: Uuid) -> Vec<u8> {
    let qr = qrcode::QrCode::new(id.to_string()).unwrap();
    let scale = 6;
    let width = qr.width() * scale;
    let mut bytes = vec![255; 800 * 600 * 3];
    for y in 0..qr.width() {
        for x in 0..qr.width() {
            if qr[(x, y)] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let i = ((300 - width / 2 + y * scale + dy) * 800 + 400 - width / 2
                            + x * scale
                            + dx)
                            * 3;
                        bytes[i..i + 3].fill(0);
                    }
                }
            }
        }
    }
    bytes
}
pub(crate) fn metadata() -> Metadata {
    Python::with_gil(|py| {
        let module = PyModule::from_code_bound(
            py,
            r#"
import json
def resolve(domain, portal):
    return json.dumps(dict(id=portal, short_id='FNPUME0T93U', name='fixture', size=40,
        organization_id=None, default_domain_id=None, redirect_url=None,
        created_at='2026-09-01T00:00:00Z',updated_at='2026-09-01T00:00:00Z'))
"#,
            "fixture.py",
            "fixture",
        )
        .unwrap();
        Metadata::new(module.getattr("resolve").unwrap().unbind())
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_detector_mapper_and_same_peer_catalog_export_two_portals() {
    let domain = Uuid::new_v4();
    let i1 = Identity::generate();
    let i2 = Identity::generate();
    let (p1, _a1) = AukiPeer::start_external(
        i1.clone(),
        support::authority(&i1, domain),
        support::direct_config(),
    )
    .await
    .unwrap();
    let (p2, _a2) = AukiPeer::start_external(
        i2.clone(),
        support::authority(&i2, domain),
        support::direct_config(),
    )
    .await
    .unwrap();
    let clock = ClockReference {
        peer_id: p1.peer_id().to_string(),
        id: "fixture-clock".into(),
        hash: "a".repeat(32),
    };
    let cfg = Config {
        domain_id: domain,
        map_id: "observed-portals".into(),
        map_frame: "map-zup".into(),
        optical_frame: "camera-optical".into(),
        optical_registry: FrameRegistryEntry::ros_optical(
            p1.peer_id().to_string(),
            "camera-optical",
        ),
        capture_clock: clock.clone(),
        publication_clock: clock,
        width: 800,
        height: 600,
        fx: 500.,
        fy: 500.,
        cx: 400.,
        cy: 300.,
    };
    let protocols = p1.protocols();
    let peer = p1.peer_id().to_string();
    let mut engine = tokio::task::spawn_blocking(move || {
        Engine::new(&peer, protocols, cfg, metadata()).unwrap()
    })
    .await
    .unwrap();
    let a = Uuid::from_u128(1);
    let b = Uuid::from_u128(2);
    for (id, x, time) in [(a, 0., 100), (b, 1., 200), (a, 0., 300)] {
        engine = tokio::task::spawn_blocking(move || {
            let pose = RigidTransform {
                from_frame_id: "camera-optical".into(),
                to_frame_id: "map-zup".into(),
                translation: [x, 0., 0.],
                rotation_wxyz: [1., 0., 0., 0.],
            };
            engine.process(image(id), time, pose).unwrap();
            engine
        })
        .await
        .unwrap();
    }
    engine = tokio::task::spawn_blocking(move || {
        let before = engine.map.snapshot_reference();
        let wrong_frame = RigidTransform::identity("unrelated-camera", "map-zup");
        assert!(engine.process(image(a), 400, wrong_frame).is_err());
        let pose = RigidTransform::identity("camera-optical", "map-zup");
        assert!(engine.process(image(a), 300, pose.clone()).is_err());
        assert!(engine.process(vec![0; 3], 400, pose).is_err());
        assert_eq!(engine.map.snapshot_reference(), before);
        engine
    })
    .await
    .unwrap();
    let client = ComponentProtocolClient::new(p2.protocols());
    let route = p1.listen_addresses()[0].clone();
    let CatalogResponse::Snapshot { snapshot: catalog } = client
        .catalog_exact(p1.peer_id(), route.clone(), None)
        .await
        .unwrap()
    else {
        panic!("catalog")
    };
    let camera = engine.capture.product();
    assert!(
        catalog
            .products
            .iter()
            .any(|p| p.manifest.reference() == camera.reference())
    );
    let PayloadContract::Camera(contract) = &camera.producer.payload else {
        panic!("camera contract")
    };
    assert_eq!(contract.encoding, "jpeg");
    let frame_registry = camera.producer.spatial_frame.as_ref().unwrap();
    frame_registry.validate().unwrap();
    assert_eq!(
        frame_registry.reference.hash,
        frame_registry.definition.hash()
    );
    let remote_camera = catalog
        .components
        .iter()
        .find(|c| c.manifest.component_id == "mapping-camera")
        .unwrap();
    assert_eq!(
        remote_camera.current_outputs["frames"]
            .manifest
            .spatial_frame
            .as_ref(),
        Some(frame_registry)
    );
    let mut video = client
        .subscribe_product_exact::<VideoFrame>(
            p1.peer_id(),
            route.clone(),
            camera.reference(),
            ObservationStart::LatestExisting,
            BufferLimits::entries(2),
            |frame| frame.bytes.len(),
        )
        .await
        .unwrap();
    assert_eq!(
        video.product().producer.spatial_frame.as_ref(),
        Some(frame_registry)
    );
    let RemoteObservationEvent::Observation(frame) = video.next().await.unwrap().unwrap() else {
        panic!("camera frame")
    };
    assert_eq!(frame.timestamp_ns, 300);
    assert_eq!(frame.payload.encoding, "jpeg");
    let decoded =
        ::image::load_from_memory_with_format(&frame.payload.bytes, ::image::ImageFormat::Jpeg)
            .unwrap();
    assert_eq!((decoded.width(), decoded.height()), (800, 600));
    assert!(frame.payload.bytes.len() < 800 * 600 * 3);
    drop(video);
    let maps: Vec<_> = auki_scenegraph::catalog::list_maps(&catalog)
        .unwrap()
        .into_iter()
        .filter(|m| m.data.portals.len() == 2)
        .collect();
    assert_eq!(maps.len(), 1);
    assert_eq!(maps[0].data.portals.len(), 2);
    assert_eq!(maps[0].snapshot.product.peer_id, p1.peer_id().to_string());
    assert!(
        maps[0]
            .data
            .portals
            .iter()
            .any(|p| p.anchor_id == a.to_string())
    );
    let mut remote = client
        .subscribe_product_exact::<MapSnapshot>(
            p1.peer_id(),
            route,
            maps[0].snapshot.product.clone(),
            ObservationStart::LatestExisting,
            BufferLimits::entries(2),
            MapSnapshot::encoded_size,
        )
        .await
        .unwrap();
    let RemoteObservationEvent::Observation(observation) = remote.next().await.unwrap().unwrap()
    else {
        panic!("snapshot")
    };
    assert_eq!(observation.sequence, maps[0].snapshot.sequence);
    observation.payload.validate().unwrap();
    let anchors = &observation.payload.scenegraph.anchors;
    assert_eq!(anchors.len(), 2);
    for anchor in anchors.values() {
        assert_eq!(anchor.side_length_m, 0.4);
        assert_eq!(anchor.pose_in_map.to_frame_id, "map-zup");
    }
    let pa = anchors[&a.to_string()].pose_in_map.translation;
    let pb = anchors[&b.to_string()].pose_in_map.translation;
    assert!((pb[0] - pa[0] - 1.).abs() < 0.02);
    assert!(pa[2] > 0.5 && pa[2] < 2.);
    if let Ok(directory) = std::env::var("GALBOT_MAPPING_ARTIFACT_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join("two-portals.usda"),
            observation
                .payload
                .scenegraph
                .to_usda_with_portal_geometry()
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join("two-portals.snapshot.json"),
            serde_json::to_vec_pretty(&*observation.payload).unwrap(),
        )
        .unwrap();
    }
    // A timestamped non-camera sensor is available through exactly the same endpoint.
    let mut sensor = sensors::Sensor::mount(
        &engine.runtime,
        engine.endpoint.as_ref().unwrap(),
        sensors::SensorSpec {
            id: "range.front".into(),
            clock: engine.cfg.capture_clock.clone(),
            frame_definition: None,
            payload: PayloadContract::Structured(StructuredPayloadContract {
                datatype: "galbot.sensor-reading".into(),
                schema: "galbot.sensor-reading/v1".into(),
                modality: "range".into(),
                observes: "front range".into(),
                unit: Some("meters".into()),
            }),
            metadata: serde_json::json!({"sensor_id":"range.front"}),
        },
    )
    .unwrap();
    let reading = r#"{"sensor_id":"range.front","measurement":{"kind":"range","distance_m":1.5}}"#;
    for timestamp in [1_000_000_000, 6_000_000_000, 12_000_000_000] {
        sensor.publish(timestamp, reading, vec![]).unwrap();
    }
    assert!(sensor.publish(11_000_000_000, reading, vec![]).is_err());
    let sensors::SensorOutput::Measurement(_, capture) = &sensor.output else {
        panic!("measurement")
    };
    let product = capture.product();
    assert_eq!(
        product.buffer().range().entries,
        2,
        "ten second history evicts the first observation"
    );
    let mut stream = client
        .subscribe_product_exact::<sensors::SensorReading>(
            p1.peer_id(),
            p1.listen_addresses()[0].clone(),
            product.reference(),
            ObservationStart::LatestExisting,
            BufferLimits::entries(2),
            |_| 128,
        )
        .await
        .unwrap();
    let RemoteObservationEvent::Observation(reading) = stream.next().await.unwrap().unwrap() else {
        panic!("sensor reading")
    };
    assert_eq!(reading.timestamp_ns, 12_000_000_000);
    assert_eq!(reading.payload.sensor_id, "range.front");
    sensor.end();
    drop(stream);
    // Actual native detection and camera Products are advertised on the same peer.
    assert_eq!(catalog.products.len(), 5);
    tokio::task::spawn_blocking(move || drop(engine))
        .await
        .unwrap();
    p2.shutdown().await.unwrap();
    p1.shutdown().await.unwrap();
}
