mod support;
#[tokio::test]
async fn calibration_is_catalog_metadata_and_resolves_only_exact_configuration()->anyhow::Result<()>{
 use spatial_home::{mobile_camera::*,mobile_products::MobileProducts,mobile_pose::CameraIntrinsics};
 use serde_json::json;
 let pair=support::LocalPair::start().await?;
 let products=MobileProducts::mount(&pair.a)?;
 let peer=pair.a.peer_id().to_string();let session=uuid::Uuid::new_v4().to_string();let id=uuid::Uuid::new_v4().to_string();
 let config=RegisteredConfiguration{id:id.clone(),configuration:Configuration{session:session.clone(),camera:CameraIntrinsics{width:640,height:480,fx:320.,fy:320.,cx:320.,cy:240.}}};
 let publisher=CalibrationPublisher::new(&products,&config)?;
 let network=json!({"peers":[{"peer_id":peer,"catalog":products.runtime.catalog().snapshot()}]});
 let frame=format!("phone-camera:{session}");
 assert_eq!(registry_calibration(&network,&peer,&id,&frame)?["width"],640);
 assert!(registry_calibration(&network,&peer,"other",&frame).is_err());
 assert!(registry_calibration(&network,&peer,&id,"other-frame").is_err());
 assert!(registry_calibration(&network,"other-peer",&id,&frame).is_err());
 assert_eq!(serde_json::to_value(CalibrationReference{configuration_id:id})?,json!({"configuration_id":config.id}));
 publisher.close()?;products.close().await?;pair.a.shutdown().await?;pair.b.shutdown().await?;Ok(())
}
