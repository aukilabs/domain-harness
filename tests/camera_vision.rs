mod support;
use anyhow::Result;
use serde_json::json;
use spatial_home::{mobile_camera::Camera,mobile_products::MobileProducts,camera_vision,sensor_preview};
use std::time::Duration;

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn snapshot_waits_for_new_frame_and_releases_camera_without_calibration()->Result<()> {
 let pair=support::LocalPair::start().await?;
 let result=async {
  let dir=tempfile::tempdir()?;let homes=spatial_home::local_scenes::LocalScenes::load(dir.path())?;
  let products=MobileProducts::mount(&pair.b)?;let mut camera=Camera::new(&products)?;
  let mut jpeg=Vec::new();image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(&image::RgbImage::from_pixel(640,480,image::Rgb([220,30,20])))?;
  camera.publish(1,jpeg.clone())?; // This retained image is not a fresh capture.
  let owner=pair.b.peer_id().to_string();let route=spatial_home::network::HomeConnection::direct(&pair.b).routes[0].clone();
  let row=json!({"peer_id":owner,"catalog_status":"ready","catalog_route":route,"catalog":products.runtime.catalog().snapshot()});
  let catalogs=std::collections::BTreeMap::from([(owner.clone(),row.clone())]);
  let choices=sensor_preview::camera_choices(&catalogs);let product=choices[0]["product_id"].clone();
  let network=json!({"peers":[row]});let (_ctx,crx)=tokio::sync::watch::channel(catalogs);
  let (tx,rx)=tokio::sync::mpsc::channel(4);let (_ltx,lrx)=tokio::sync::watch::channel(Default::default());
  let stop=tokio_util::sync::CancellationToken::new();let ar=Default::default();
  let exercise=async {
   let work:Result<()>=async {
    assert!(!camera.demanded());
    let capture=camera_vision::capture(&tx,&network,json!({"peer_id":owner,"product_id":product}));tokio::pin!(capture);
    let publish=async {
     tokio::time::timeout(Duration::from_secs(3),async {while !camera.demanded(){tokio::time::sleep(Duration::from_millis(20)).await;}}).await?;
     tokio::time::sleep(Duration::from_millis(150)).await;
     camera.publish(2,jpeg)?;Ok::<_,anyhow::Error>(())
    };
    let (frame,sent)=tokio::join!(&mut capture,publish);sent?;let frame=frame?;
    assert_eq!(frame["capture_timestamp_ns"],2);assert_eq!(frame["peer_id"],owner);assert!(frame["image"].as_str().unwrap().len()>100);
    tokio::time::timeout(Duration::from_secs(3),async {while camera.demanded(){tokio::time::sleep(Duration::from_millis(20)).await;}}).await?;
    assert!(camera_vision::capture(&tx,&network,json!({"peer_id":owner,"product_id":"missing"})).await.is_err());
    // Only old retained frames now: never serve them as the current view.
    assert!(camera_vision::capture(&tx,&network,json!({"peer_id":owner,"product_id":product})).await.is_err());
    Ok(())
   }.await;stop.cancel();work
  };
  let (_,result)=tokio::join!(sensor_preview::serve(&pair.a,crx,rx,lrx,&ar,&homes,&stop),exercise);
  camera.close()?;products.close().await?;result
 }.await;
 pair.shutdown().await?;result
}
