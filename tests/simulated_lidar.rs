mod support;
use anyhow::Result;
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ObservationStart, RemoteObservationEvent,
};
use auki_components::{BufferLimits, ProductReference};
use spatial_home::{
    simulated_camera::Camera,
    simulated_lidar::{Lidar, PRODUCT, Scan},
};
use std::time::Duration;

fn scan() -> Scan {
    Scan {
        timestamp_ns: 1,
        ranges_m: vec![Some(2.0); 1936],
        world_position: [0.0, 0.65, -0.28],
        world_rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
    }
}
#[test]
fn validates_scan_and_preserves_no_returns() {
    let mut s = scan();
    s.validate().unwrap();
    assert_eq!(s.local_hits().len(), 1936);
    s.ranges_m[0] = None;
    assert_eq!(s.local_hits().len(), 1935);
    s.ranges_m[1] = Some(10.1);
    assert!(s.validate().is_err());
    s.ranges_m[1] = Some(f32::NAN);
    assert!(s.validate().is_err());
    s.ranges_m[1] = Some(2.0);
    s.ranges_m.pop();
    assert!(s.validate().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lidar_subscription_does_not_activate_camera() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let camera = Camera::mount(&pair.b)?;
    let mut lidar = Lidar::mount(&camera.runtime, &camera.endpoint)?;
    let client = ComponentProtocolClient::new(pair.a.protocols());
    let route = pair.b.listen_addresses()[0].clone();
    let CatalogResponse::Snapshot { snapshot } = client
        .catalog_exact(pair.b.peer_id(), route.clone(), None)
        .await?
    else {
        panic!("Expected catalog")
    };
    assert_eq!(snapshot.components.len(), 2);
    assert_eq!(snapshot.products.len(), 2);
    assert!(!camera.demanded() && !lidar.demanded(&camera.endpoint));
    let p = snapshot
        .products
        .iter()
        .find(|p| p.manifest.product_id == PRODUCT)
        .unwrap();
    let mut subscription = client
        .subscribe_product_exact(
            pair.b.peer_id(),
            route,
            ProductReference {
                peer_id: pair.b.peer_id().to_string(),
                product_id: PRODUCT.into(),
                manifest_hash: p.manifest_hash.clone(),
            },
            ObservationStart::NewOnly,
            BufferLimits::entries(1),
            |s: &Scan| s.ranges_m.len() * 8,
        )
        .await?;
    assert!(lidar.demanded(&camera.endpoint));
    assert!(
        !camera.demanded(),
        "LiDAR-only consumers must not trigger camera rendering"
    );
    lidar.publish(scan())?;
    let event = tokio::time::timeout(Duration::from_secs(3), subscription.next()).await??;
    let Some(RemoteObservationEvent::Observation(o)) = event else {
        panic!("Expected scan")
    };
    assert_eq!(o.payload.ranges_m.len(), 1936);
    assert_eq!(o.timestamp_ns, 1);
    assert!(lidar.publish(scan()).is_err());
    subscription.close().await?;
    tokio::time::timeout(Duration::from_secs(3), async {
        while lidar.demanded(&camera.endpoint) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    assert!(!camera.demanded());
    lidar.close();
    camera.close().await?;
    pair.shutdown().await?;
    Ok(())
}
