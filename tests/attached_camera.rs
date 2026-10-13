mod support;
use anyhow::Result;
use auki_components::*;
use auki_odometry::{OdometryComponent,PoseContract,Tracking,capture_pose_history};
use serde_json::json;
use spatial_home::{sensor_preview,view_camera};
use std::time::Duration;

async fn query(tx:&tokio::sync::mpsc::Sender<sensor_preview::Request>,id:&str,hash:&str)->Result<serde_json::Value>{
 let (reply,receive)=tokio::sync::oneshot::channel();
 tx.send(sensor_preview::Request{sensor:Some(id.into()),follow:true,capture:false,expected_manifest:Some(hash.into()),scene:false,ar:false,pick:false,reply}).await?;
 tokio::time::timeout(Duration::from_secs(10),receive).await??
}

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn typed_frame_attachment_uses_new_pose_stream_without_video_and_rejects_lost_tracking()->Result<()> {
 let pair=support::LocalPair::start().await?;
 let result=async {
  let dir=tempfile::tempdir()?;let homes=support::seeded_local_scenes(dir.path())?;
  let home=homes.snapshot("humanoid-hub-conference")?;let frame=home.scenegraph.map.frame.id.clone();
  let owner=pair.b.peer_id().to_string();let products=spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
  let contract=PoseContract{from_frame:auki_registry::FrameRegistryEntry::ros_optical(&owner,"head-optical"),
   to_frame:spatial_home::conventions::registry(&pair.a.peer_id().to_string(),&frame)?,
   clock:spatial_home::model::clock_ref(&spatial_home::model::clock_entry(&owner,"session")),session_id:"session".into()};
  let mut publisher=OdometryComponent::new(&products.runtime,"sensor-pose",contract)?;
  let capture=capture_pose_history(&products.runtime,"head-pose-history",&publisher,BufferLimits{max_entries:Some(8),max_bytes:Some(64*1024),target_duration:None})?;
  products.endpoint.export_product(&capture.product())?;
  publisher.publish(1,Tracking::Valid{pose:spatial_home::local_mapper::identity()})?;
  let route=spatial_home::network::HomeConnection::direct(&pair.b).routes[0].clone();
  let row=json!({"peer_id":owner,"catalog_status":"ready","catalog_route":route,"catalog":products.runtime.catalog().snapshot()});
  let catalogs=std::collections::BTreeMap::from([(owner.clone(),row.clone())]);
  assert!(sensor_preview::camera_sensors(&catalogs).is_empty(),"No video camera needed");
  let network=json!({"peers":[row],"aliases":{owner.clone():"Dan's Phone"}});
  let viewer=serde_json::from_value(json!({"frame":frame,"mode":"3d","screen_right":[1,0,0]}))?;
  let action=view_camera::attach_action(&network,json!({"peer_id":owner,"frame_id":"head-optical"}),"local:humanoid-hub-conference",&frame,Some(&viewer))?;
  assert!(view_camera::attach_action(&network,json!({"peer_id":owner,"frame_id":"wrong"}),"local:humanoid-hub-conference",&frame,Some(&viewer)).is_err());
  let id=action["sensor_id"].as_str().unwrap();let hash=action["pose_product"]["manifest_hash"].as_str().unwrap();
  let (_catalog_tx,catalog_rx)=tokio::sync::watch::channel(catalogs);
  let (tx,rx)=tokio::sync::mpsc::channel(4);let (_live_tx,live_rx)=tokio::sync::watch::channel(Default::default());
  let stop=tokio_util::sync::CancellationToken::new();let ar=Default::default();
  let work=async {
   let result:Result<()>=async {
    assert!(query(&tx,id,"wrong-manifest").await.is_err());
    assert!(query(&tx,id,hash).await.unwrap_err().to_string().contains("Waiting for fresh"),"Retained pose must not be treated as live");
    let mut received=None;let mut last_error=String::new();
    for n in 2..150 {
     publisher.publish(n,Tracking::Valid{pose:auki_odometry::Pose{translation:[1.,2.,3.],..spatial_home::local_mapper::identity()}})?;
     tokio::time::sleep(Duration::from_millis(30)).await;
     match query(&tx,id,hash).await {Ok(value)=>{received=Some(value);break;},Err(error)=>last_error=error.to_string()}
    }
    let value=received.unwrap_or_else(||panic!("fresh subscribed pose: {last_error}"));assert_eq!(value["frame"],frame);assert_eq!(value["position"],json!([1.,2.,3.]));
    assert_eq!(value["look_at"],json!([1.,2.,4.]));assert!(value.get("data").is_none());
    publisher.publish(200,Tracking::Lost)?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(query(&tx,id,hash).await.unwrap_err().to_string().contains("tracking unavailable"));
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert!(query(&tx,id,hash).await.unwrap_err().to_string().contains("stale"));
    Ok(())
   }.await;stop.cancel();result
  };
  let (_,result)=tokio::join!(sensor_preview::serve(&pair.a,catalog_rx,rx,live_rx,&ar,&homes,&stop),work);
  result?;products.close().await?;Ok(())
 }.await;
 pair.a.shutdown().await?;pair.b.shutdown().await?;result
}
