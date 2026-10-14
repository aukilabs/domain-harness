//! Demand-driven camera snapshots for local visual questions, not scene edits.
use anyhow::{ensure,Result};
use base64::{Engine,engine::general_purpose::STANDARD};
use serde_json::{Value,json};

pub fn tool()->Value {
 json!({"type":"function","function":{"name":"capture_camera_frame","description":"Take one fresh image from a discovered peer camera so you can describe what it sees. Resolve alias and camera product through inspect_catalog first; choose the best matching camera for the user's wording. Exact peer_id and product_id required. No PiP, calibration or hardware movement. Unavailable cameras fail, never substitute another camera or old image.","parameters":{"type":"object","properties":{"peer_id":{"type":"string"},"product_id":{"type":"string"}},"required":["peer_id","product_id"],"additionalProperties":false}}})
}

pub async fn capture(tx:&tokio::sync::mpsc::Sender<crate::sensor_preview::Request>,network:&Value,args:Value)->Result<Value>{
 ensure!(args["product_id"].as_str().is_some_and(|s|!s.is_empty()),"Select an exact camera product");
 let selected=crate::catalog_chat::camera_action(network,args)?;
 let (reply,receive)=tokio::sync::oneshot::channel();
 tx.try_send(crate::sensor_preview::Request{home_frame:None,debug_pose:false,sensor:Some(selected["sensor_id"].as_str().unwrap().into()),capture:true,follow:false,scene:false,ar:false,pick:false,expected_manifest:None,reply})
  .map_err(|_|anyhow::anyhow!("Camera request queue unavailable"))?;
 Ok(tokio::time::timeout(std::time::Duration::from_secs(10),receive).await
  .map_err(|_|anyhow::anyhow!("Camera did not provide a fresh frame in time"))?
  .map_err(|_|anyhow::anyhow!("Camera connection stopped"))??.value)
}

/// Validate bytes before allocating, bound JPEG decoding, then normalize all
/// supported SDK encodings to a modest JPEG understood by the vision model.
pub fn encode(f:&auki_components::VideoFrame)->Result<String>{
 ensure!(f.width>0&&f.height>0&&f.width<=4096&&f.height<=4096&&f.bytes.len()<=8*1024*1024,"Camera image exceeds limits");
 let pixels=f.width as usize*f.height as usize;
 let image=match f.encoding.as_str(){
  "rgb8"=>{ensure!(f.bytes.len()==pixels*3,"Invalid RGB image length");image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(f.width,f.height,f.bytes.to_vec()).unwrap())},
  "luma8"=>{ensure!(f.bytes.len()==pixels,"Invalid grayscale image length");image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(f.width,f.height,f.bytes.to_vec()).unwrap())},
  "jpeg"|"jpg"=>{
   let mut reader=image::ImageReader::with_format(std::io::Cursor::new(f.bytes.as_ref()),image::ImageFormat::Jpeg);
   let mut limits=image::Limits::default();limits.max_image_width=Some(4096);limits.max_image_height=Some(4096);limits.max_alloc=Some(128*1024*1024);reader.limits(limits);
   let image=reader.decode()?;ensure!(image.width()==f.width&&image.height()==f.height,"Camera JPEG dimensions disagree with its contract");image
  },_=>anyhow::bail!("Unsupported camera encoding: {}",f.encoding)
 };
 let image=if image.width()>1024||image.height()>1024{image.thumbnail(1024,1024)}else{image};
 let image=image.to_rgb8();let mut jpeg=Vec::new();
 image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg,85).encode_image(&image)?;
 ensure!(jpeg.len()<=2*1024*1024,"Vision image exceeds limit");Ok(STANDARD.encode(jpeg))
}

#[cfg(test)] mod tests {
 use super::*;
 #[test] fn validates_and_normalizes_camera_encodings()->Result<()> {
  let mut f=auki_components::VideoFrame{width:8,height:6,encoding:"rgb8".into(),bytes:vec![180;8*6*3].into()};
  let jpeg=STANDARD.decode(encode(&f)?)?;assert_eq!(image::load_from_memory(&jpeg)?.width(),8);
  f.encoding="jpeg".into();f.bytes=jpeg.into();assert!(encode(&f).is_ok());f.width=9;assert!(encode(&f).is_err());
  f.width=8;f.encoding="luma8".into();f.bytes=vec![80;48].into();assert!(encode(&f).is_ok());f.bytes=vec![80;47].into();assert!(encode(&f).is_err());
  f.width=5000;assert!(encode(&f).is_err());Ok(())
 }
}
