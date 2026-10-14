#[allow(dead_code)] mod support;
#[allow(dead_code)] #[path="../diagnostics/connection-peer/src/diagnostic_log.rs"] mod diagnostic_log;
mod compact_pose {pub use spatial_home::compact_pose::*;}
mod camera_frame {pub use spatial_home::camera_frame::*;}
mod qr_decode {pub use spatial_home::qr_decode::*;}
mod fisheye {pub use spatial_home::fisheye::*;}
#[allow(dead_code)] #[path="../diagnostics/connection-peer/src/alignment.rs"] mod alignment;
#[allow(dead_code)] #[path="../diagnostics/connection-peer/src/home.rs"] mod home;
#[allow(dead_code)] #[derive(Clone)]
struct Home {schema:String,domain_id:String,peer_id:String,routes:Vec<String>,frame:String,product_id:String}
#[allow(dead_code)] #[path="../diagnostics/connection-peer/src/poses.rs"] mod poses;
#[allow(dead_code)] #[path="../diagnostics/connection-peer/src/camera.rs"] mod camera;

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn diagnostic_jpeg_reaches_generic_home_preview_unchanged()->anyhow::Result<()> {
    use spatial_home::{sensor_preview::{self,Request},device_catalogs};
    let pair=support::LocalPair::start().await?;
    let p=poses::Publisher::mount(&pair.b)?;
    let mut camera=camera::Camera::mount(&p,camera::Calibration {width:64,height:64,k:[60.,60.,32.,32.],d:[0.;4],
        camera_to_base:auki_odometry::Pose {translation:[0.,0.,1.],rotation_xyzw:[0.,0.,0.,1.]}})?;
    let mut jpeg=vec![];image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(&image::GrayImage::new(64,64))?;
    let timestamp=9_007_199_254_740_993;
    camera.publish(timestamp,jpeg.clone())?;
    let calibration = camera.localization_calibration()?;
    assert_eq!(calibration.camera_product, camera.product().reference());
    assert_eq!(calibration.clock, p.contract.clock);
    assert_eq!(calibration.camera_frame_id, camera.product().producer.spatial_frame_id.clone().unwrap());
    let home = alignment::tests::scene();
    let (_home_tx, home_rx) = tokio::sync::watch::channel(Some(home.clone()));
    let alignment = alignment::Worker::mount(&p, &camera, home.clone(), home_rx)?;
    assert_eq!(alignment.target, (&home.frame).into());
    let routes=pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
    let mut row=device_catalogs::fetch(&pair.a,pair.b.peer_id(),&routes).await;
    row["registries"]=spatial_home::product_registry::resolve(&pair.a,pair.b.peer_id(),&row,None,&Default::default()).await;
    let catalogs=device_catalogs::Catalogs::from([(pair.b.peer_id().to_string(),row)]);
    let (_catalog_tx,catalog_rx)=tokio::sync::watch::channel(catalogs);
    let (tx,rx)=tokio::sync::mpsc::channel(4);
    let (_live_tx,live)=tokio::sync::watch::channel(Default::default());
    let stop=tokio_util::sync::CancellationToken::new();
    let dir=tempfile::tempdir()?;let homes=spatial_home::local_scenes::LocalScenes::load(dir.path())?;
    let frames=Default::default();let held=Default::default();
    let serve=sensor_preview::serve(&pair.a,catalog_rx,rx,live,&frames,&homes,&held,&stop);
    let exercise=async {
        let value=tokio::time::timeout(std::time::Duration::from_secs(5),async {
            loop {
                let (reply,answer)=tokio::sync::oneshot::channel();
                tx.send(Request {home_frame:None,debug_pose:false,sensor:Some(format!("{}/bbos-head-camera-history",pair.b.peer_id())),scene:false,
                    ar:false,pick:false,follow:false,capture:false,expected_manifest:None,reply}).await?;
                if let Ok(frame)=answer.await? {return Ok::<_,anyhow::Error>(frame);}
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await??;
        assert_eq!(value["capture_timestamp_ns"],timestamp.to_string());
        assert_eq!(value.image.as_deref(),Some(jpeg.as_slice()));assert!(value.get("data").is_none());
        assert_eq!(value["clock"],serde_json::to_value(&p.contract.clock)?);
        assert_eq!(value["metadata"]["value"]["frame"],value["spatial_frame"]["reference"]);
        let packet=camera_frame::browser_packet(&value.value,value.image.as_ref().unwrap())?;
        let len=u32::from_le_bytes(packet[..4].try_into().unwrap()) as usize;
        assert_eq!(&packet[4+len..],jpeg);
        Ok::<(),anyhow::Error>(())
    };
    let (_,result)=tokio::join!(serve,async {let r=exercise.await;stop.cancel();r});
    drop(alignment);drop(camera);p.endpoint.close().await?;p.registry.close().await?;pair.shutdown().await?;result
}
