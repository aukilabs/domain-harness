mod support;
use anyhow::Result;
use auki_components::*;
use auki_odometry::{OdometryComponent,PoseContract,Tracking,capture_pose_history};
use serde_json::json;
use spatial_home::{sensor_preview,view_camera};

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn legacy_pose_products_cannot_enable_camera_following()->Result<()> {
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
  assert!(sensor_preview::frame_sensors(&catalogs).is_empty(), "legacy frame following stays unavailable");
  assert!(view_camera::attach_action(&network,json!({"peer_id":owner,"frame_id":"head-optical"}),"local:humanoid-hub-conference",&frame,Some(&viewer)).is_err());
  products.close().await?;Ok(())
 }.await;
 pair.a.shutdown().await?;pair.b.shutdown().await?;result
}
