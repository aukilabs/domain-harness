#[allow(dead_code)]
mod support;
mod compact_pose { pub use spatial_home::compact_pose::*; }
mod depth_points { pub use spatial_home::depth_points::*; }
mod component_enabled { pub use spatial_home::component_enabled::*; }
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
        translation:[0.2,0.,0.3],rotation_xyzw:[0.,0.,0.,1.]}},pair.a.peer_id().to_string())?;
    assert!(!depth.wants_capture(&publisher.endpoint));
    depth.publish(depth::Capture {timestamp_ns:123456788,points:vec![[0.,0.,2.]],colors:None},&publisher.endpoint)?;
    let routes=pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
    let mut row=spatial_home::device_catalogs::fetch(&pair.a,pair.b.peer_id(),&routes).await;
    row["registries"]=spatial_home::product_registry::resolve(&pair.a,pair.b.peer_id(),&row,None,&Default::default()).await;
    assert_eq!(row["registries"]["status"],"ready");
    let catalog:CatalogSnapshot=serde_json::from_value(row["catalog"].clone())?;
    let entry=catalog.products.iter().find(|p|p.manifest.product_id==depth::PRODUCT).unwrap();
    assert!(entry.metadata.is_none(),"Discovery must work before the first demanded capture");
    let client=ComponentProtocolClient::new(pair.a.protocols());
    let route:auki_sdk::Multiaddr=row["catalog_route"].as_str().unwrap().parse()?;
    let mut subscription=client.subscribe_product_exact_with_decoder(pair.b.peer_id(),route.clone(),entry.manifest.reference(),
        ObservationStart::NewOnly,BufferLimits {max_entries:Some(1),max_bytes:Some(128*1024),target_duration:None},
        |v:&depth_points::PackedPoints|v.as_bytes().len(),depth_points::PackedPoints::decoder()?).await?;
    assert!(depth.wants_capture(&publisher.endpoint));
    depth.publish(depth::Capture {timestamp_ns:123456789,points:vec![[0.,0.,2.]],colors:Some(vec![[10,20,30]])},&publisher.endpoint)?;
    assert!(depth.publish(depth::Capture {timestamp_ns:123456789,points:vec![[0.,0.,2.]],colors:None},&publisher.endpoint).is_err());
    tokio::time::timeout(std::time::Duration::from_secs(2),subscription.next()).await??;
    let data=client.observations_exact_with_decoder(pair.b.peer_id(),route.clone(),
        ObservationRequest {product:entry.manifest.reference(),selection:ObservationSelection::LatestExisting},depth_points::PackedPoints::decoder()?).await?;
    let contract=data.observations[0].payload.contract();
    contract.validate_packed_output(&data.producer)?;
    assert_eq!(contract.clock,publisher.contract.clock);
    assert_eq!(contract.session_id,publisher.contract.session_id);
    assert_eq!(data.observations[0].timestamp_ns,123456789);
    assert_eq!(data.observations[0].payload.as_ref().clone().into_points().colors,Some(vec![[10,20,30]]));
    let mount=catalog.products.iter().find(|p|p.manifest.product_id=="bbos-depth-extrinsic").unwrap();
    let c:compact_pose::Contract=serde_json::from_value(mount.metadata.as_ref().unwrap().value.clone())?;
    assert_eq!(c.from_frame,contract.frame);
    assert_eq!(c.to_frame,(&publisher.contract.from_frame).into());
    let component=catalog.components.iter().find(|c|c.manifest.component_id=="bbos-depth").unwrap();
    let mut catalogs=spatial_home::device_catalogs::Catalogs::new();
    catalogs.insert(pair.b.peer_id().to_string(),row.clone());
    let command=|enabled|spatial_home::component_control::Command {peer_id:pair.b.peer_id().to_string(),
        component_id:"bbos-depth".into(),manifest_hash:component.manifest_hash.clone(),enabled};
    assert!(!spatial_home::component_control::invoke(&client,&catalogs,command(false)).await?.enabled);
    depth.publish(depth::Capture {timestamp_ns:223456789,points:vec![[0.,0.,3.]],colors:None},&publisher.endpoint)?;
    assert!(!depth.wants_capture(&publisher.endpoint));
    let paused=client.observations_exact_with_decoder(pair.b.peer_id(),route.clone(),
        ObservationRequest {product:entry.manifest.reference(),selection:ObservationSelection::LatestExisting},depth_points::PackedPoints::decoder()?).await?;
    assert_eq!(paused.observations[0].sequence,data.observations[0].sequence);
    assert!(spatial_home::component_control::invoke(&client,&catalogs,command(true)).await?.enabled);
    depth.publish(depth::Capture {timestamp_ns:323456789,points:vec![[0.,0.,4.]],colors:None},&publisher.endpoint)?;
    let resumed=client.observations_exact_with_decoder(pair.b.peer_id(),route.clone(),
        ObservationRequest {product:entry.manifest.reference(),selection:ObservationSelection::LatestExisting},depth_points::PackedPoints::decoder()?).await?;
    assert!(resumed.observations[0].sequence>data.observations[0].sequence);
    subscription.close().await?;
    tokio::time::timeout(std::time::Duration::from_secs(2),async {
        while depth.wants_capture(&publisher.endpoint){tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
    }).await?;
    depth.publish(depth::Capture {timestamp_ns:423456789,points:vec![[0.,0.,5.]],colors:None},&publisher.endpoint)?;
    let idle=client.observations_exact_with_decoder(pair.b.peer_id(),route,
        ObservationRequest {product:entry.manifest.reference(),selection:ObservationSelection::LatestExisting},depth_points::PackedPoints::decoder()?).await?;
    assert_eq!(idle.observations[0].sequence,resumed.observations[0].sequence,"Finite fetch is not subscription demand");
    depth.close(&publisher)?;
    publisher.endpoint.close().await?;
    publisher.registry.close().await?;
    pair.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn authenticated_peer_other_than_paired_home_cannot_disable_depth()->anyhow::Result<()> {
    let pair=support::LocalPair::start().await?;
    let publisher=poses::Publisher::mount(&pair.b)?;
    let depth=depth::Depth::mount(&publisher,depth::Configuration {sensor_to_base:auki_odometry::Pose {
        translation:[0.,0.,0.],rotation_xyzw:[0.,0.,0.,1.]}},pair.b.peer_id().to_string())?;
    let routes=pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
    let row=spatial_home::device_catalogs::fetch(&pair.a,pair.b.peer_id(),&routes).await;
    let catalog:auki_components::CatalogSnapshot=serde_json::from_value(row["catalog"].clone())?;
    let component=catalog.components.iter().find(|c|c.manifest.component_id=="bbos-depth").unwrap();
    let command=spatial_home::component_control::Command {peer_id:pair.b.peer_id().to_string(),
        component_id:"bbos-depth".into(),manifest_hash:component.manifest_hash.clone(),enabled:false};
    let mut catalogs=spatial_home::device_catalogs::Catalogs::new();catalogs.insert(pair.b.peer_id().to_string(),row);
    let error=spatial_home::component_control::invoke(&auki_component_protocol::ComponentProtocolClient::new(pair.a.protocols()),&catalogs,command).await.unwrap_err();
    assert!(error.to_string().contains("authorized"),"{error}");
    depth.close(&publisher)?;publisher.endpoint.close().await?;publisher.registry.close().await?;
    pair.shutdown().await?;Ok(())
}
