#[allow(dead_code)]
mod support;
mod compact_pose { pub use spatial_home::compact_pose::*; }
mod depth_points { pub use spatial_home::depth_points::*; }
#[path = "../diagnostics/connection-peer/src/poses.rs"]
mod poses;
#[allow(dead_code)]
#[path = "../diagnostics/connection-peer/src/depth.rs"]
mod depth;

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn actual_diagnostic_depth_is_discoverable_and_preserves_capture_identity() -> anyhow::Result<()> {
    use auki_components::*;
    use auki_component_protocol::*;
    let pair=support::LocalPair::start().await?;
    let publisher=poses::Publisher::mount(&pair.b)?;
    let mut depth=depth::Depth::mount(&publisher,depth::Configuration {sensor_to_base:auki_odometry::Pose {
        translation:[0.2,0.,0.3],rotation_xyzw:[0.,0.,0.,1.]}})?;
    depth.publish(depth::Capture {timestamp_ns:123456789,points:vec![[0.,0.,2.]],colors:Some(vec![[10,20,30]])})?;
    assert!(depth.publish(depth::Capture {timestamp_ns:123456789,points:vec![[0.,0.,2.]],colors:None}).is_err());
    let routes=pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
    let mut row=spatial_home::device_catalogs::fetch(&pair.a,pair.b.peer_id(),&routes).await;
    row["registries"]=spatial_home::product_registry::resolve(&pair.a,pair.b.peer_id(),&row,None,&Default::default()).await;
    assert_eq!(row["registries"]["status"],"ready");
    let catalog:CatalogSnapshot=serde_json::from_value(row["catalog"].clone())?;
    let entry=catalog.products.iter().find(|p|p.manifest.product_id==depth::PRODUCT).unwrap();
    let contract:depth_points::PointsContract=serde_json::from_value(entry.metadata.as_ref().unwrap().value.clone())?;
    let client=ComponentProtocolClient::new(pair.a.protocols());
    let data=client.observations_exact::<depth_points::Points>(pair.b.peer_id(),row["catalog_route"].as_str().unwrap().parse()?,
        ObservationRequest {product:entry.manifest.reference(),selection:ObservationSelection::LatestExisting}).await?;
    contract.validate_output(&data.producer)?;
    assert_eq!(contract.clock,publisher.contract.clock);
    assert_eq!(contract.session_id,publisher.contract.session_id);
    assert_eq!(data.observations[0].timestamp_ns,123456789);
    assert_eq!(data.observations[0].payload.colors,Some(vec![[10,20,30]]));
    let mount=catalog.products.iter().find(|p|p.manifest.product_id=="bbos-depth-extrinsic").unwrap();
    let c:compact_pose::Contract=serde_json::from_value(mount.metadata.as_ref().unwrap().value.clone())?;
    assert_eq!(c.from_frame,contract.frame);
    assert_eq!(c.to_frame,(&publisher.contract.from_frame).into());
    depth.close(&publisher)?;
    publisher.endpoint.close().await?;
    publisher.registry.close().await?;
    pair.shutdown().await?;
    Ok(())
}
