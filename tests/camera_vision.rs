mod support;
use anyhow::Result;
use serde_json::json;
use spatial_home::{mobile_camera::Camera,mobile_products::MobileProducts,camera_vision,sensor_preview};
use std::time::Duration;

async fn preview(tx:&tokio::sync::mpsc::Sender<sensor_preview::Request>,sensor:Option<String>)->Result<sensor_preview::Reply> {
 let (reply,answer)=tokio::sync::oneshot::channel();
 tx.send(sensor_preview::Request {home_frame:None,debug_pose:false,sensor,scene:false,ar:false,pick:false,follow:false,capture:false,expected_manifest:None,reply}).await?;
 answer.await?
}

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
     camera.publish(2,jpeg.clone())?;Ok::<_,anyhow::Error>(())
    };
    let (frame,sent)=tokio::join!(&mut capture,publish);sent?;let frame=frame?;
    assert_eq!(frame["capture_timestamp_ns"],2);assert_eq!(frame["peer_id"],owner);assert!(frame["image"].as_str().unwrap().len()>100);
    assert_eq!(frame["clock"],json!(spatial_home::model::clock_ref(&products.timing.clock.registry_entry())));
    assert_eq!(frame["retention"]["entries"],1);
    assert_eq!(frame["retention"]["first_timestamp_ns"],"2");
    tokio::time::timeout(Duration::from_secs(3),async {while camera.demanded(){tokio::time::sleep(Duration::from_millis(20)).await;}}).await?;
    assert_eq!(products.endpoint.active_subscriptions(),0,"Retained snapshots do not keep cameras active");
    // A second one-shot keeps both exact source samples in the same SDK buffer.
    let capture=camera_vision::capture(&tx,&network,json!({"peer_id":owner,"product_id":product}));
    let publish=async {
     tokio::time::timeout(Duration::from_secs(3),async {while !camera.demanded(){tokio::time::sleep(Duration::from_millis(20)).await;}}).await?;
     tokio::time::sleep(Duration::from_millis(150)).await;
     camera.publish(3,jpeg.clone())?;Ok::<_,anyhow::Error>(())
    };
    let (next,sent)=tokio::join!(capture,publish);sent?;let next=next?;
    assert_eq!(next["capture_timestamp_ns"],3);
    assert_eq!(next["sequence"].as_u64(),frame["sequence"].as_u64().map(|s|s+1));
    assert_eq!(next["retention"]["entries"],2);
    assert_eq!(next["retention"]["first_timestamp_ns"],"2");
    tokio::time::timeout(Duration::from_secs(3),async {while camera.demanded(){tokio::time::sleep(Duration::from_millis(20)).await;}}).await?;
    assert!(camera_vision::capture(&tx,&network,json!({"peer_id":owner,"product_id":"missing"})).await.is_err());
    // Only old retained frames now: never serve them as the current view.
    assert!(camera_vision::capture(&tx,&network,json!({"peer_id":owner,"product_id":product})).await.is_err());
    Ok(())
   }.await;stop.cancel();work
  };
  let held=spatial_home::frame_alignment::HeldTransforms::default();
  let (_,result)=tokio::join!(sensor_preview::serve(&pair.a,crx,rx,lrx,&ar,&homes,&held,&stop),exercise);
  camera.close()?;products.close().await?;result
 }.await;
 pair.shutdown().await?;result
}

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn snapshot_reuses_open_pip_and_does_not_block_preview_requests()->Result<()> {
 let pair=support::LocalPair::start().await?;
 let result=async {
  let dir=tempfile::tempdir()?;let homes=spatial_home::local_scenes::LocalScenes::load(dir.path())?;
  let products=MobileProducts::mount(&pair.b)?;let mut camera=Camera::new(&products)?;
  let mut jpeg=Vec::new();image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(&image::RgbImage::from_pixel(640,480,image::Rgb([80,90,100])))?;
  camera.publish(1,jpeg.clone())?;
  let owner=pair.b.peer_id().to_string();let route=spatial_home::network::HomeConnection::direct(&pair.b).routes[0].clone();
  let row=json!({"peer_id":owner,"catalog_status":"ready","catalog_route":route,"catalog":products.runtime.catalog().snapshot()});
  let catalogs=std::collections::BTreeMap::from([(owner.clone(),row.clone())]);
  let choices=sensor_preview::camera_choices(&catalogs);let product=choices[0]["product_id"].clone();let id=choices[0]["id"].as_str().unwrap().to_owned();
  let network=json!({"peers":[row]});let (_ctx,crx)=tokio::sync::watch::channel(catalogs);
  let (tx,rx)=tokio::sync::mpsc::channel(4);let (_ltx,lrx)=tokio::sync::watch::channel(Default::default());
  let stop=tokio_util::sync::CancellationToken::new();let ar=Default::default();
  let exercise=async {
   let work:Result<()>=async {
    let _=preview(&tx,Some(id.clone())).await;
    let old=tokio::time::timeout(Duration::from_secs(3),async {
     loop {if let Ok(frame)=preview(&tx,Some(id.clone())).await {break frame;}tokio::time::sleep(Duration::from_millis(20)).await;}
    }).await?;
    assert_eq!(old["capture_timestamp_ns"],"1");
    assert_eq!(products.endpoint.active_subscriptions(),1);
    let capture=camera_vision::capture(&tx,&network,json!({"peer_id":owner,"product_id":product}));tokio::pin!(capture);
    assert!(tokio::time::timeout(Duration::from_millis(150),&mut capture).await.is_err(),"Cached PiP frame is not a fresh snapshot");
    assert_eq!(products.endpoint.active_subscriptions(),1,"Snapshot must reuse PiP subscription");
    // The server remains responsive while the snapshot waits for its next frame.
    let listing=tokio::time::timeout(Duration::from_secs(1),preview(&tx,None)).await??;
    assert_eq!(listing["sensors"].as_array().unwrap().len(),1);
    camera.publish(2,jpeg)?;
    let frame=capture.await?;
    assert_eq!(frame["capture_timestamp_ns"],2);
    assert_eq!(frame["clock"],old["clock"]);
    assert_eq!(frame["retention"]["entries"],2);
    assert_eq!(products.endpoint.active_subscriptions(),1,"Snapshot completion must not close PiP");
    assert_eq!(preview(&tx,Some(id)).await?["capture_timestamp_ns"],"2");
    Ok(())
   }.await;stop.cancel();work
  };
  let held=spatial_home::frame_alignment::HeldTransforms::default();
  let (_,result)=tokio::join!(sensor_preview::serve(&pair.a,crx,rx,lrx,&ar,&homes,&held,&stop),exercise);
  camera.close()?;products.close().await?;result
 }.await;
 pair.shutdown().await?;result
}
