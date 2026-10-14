//! Qwen camera geometry uses the same capture-matched consumer as PiP.
use anyhow::{anyhow,ensure,Result};
use serde::Deserialize;
use serde_json::{json,Value};
#[derive(Deserialize)] #[serde(deny_unknown_fields)]
pub struct Query {pub peer_id:String,pub product_id:Option<String>,pub pixel:Option<[f64;2]>}

pub async fn geometry(tx:&tokio::sync::mpsc::Sender<crate::sensor_preview::Request>,network:&Value,args:Value,frame:&str)->Result<Value>{
    let q:Query=serde_json::from_value(args)?;
    let selected=crate::catalog_chat::camera_action(network,json!({"peer_id":q.peer_id,"product_id":q.product_id}))?;
    let work=async {
        loop {
            let (reply,receive)=tokio::sync::oneshot::channel();
            tx.try_send(crate::sensor_preview::Request{home_frame:Some(frame.into()),debug_pose:false,sensor:Some(selected["sensor_id"].as_str().unwrap().into()),
                expected_manifest:None,scene:false,ar:false,pick:true,follow:false,capture:false,reply}).map_err(|_|anyhow!("Camera request queue unavailable"))?;
            match receive.await.map_err(|_|anyhow!("Camera consumer stopped"))? {
                Err(e) if e.to_string()=="Connecting camera subscription"=>tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                result=>return result,
            }
        }
    };
    let value=tokio::time::timeout(std::time::Duration::from_secs(8),work).await.map_err(|_|anyhow!("Camera geometry timed out"))??;
    ray(&value,q.pixel,frame)
}

pub fn ray(value:&Value,pixel:Option<[f64;2]>,frame:&str)->Result<Value>{
    let geometry:crate::picking::CameraGeometry=serde_json::from_value(value["picking"].clone())
        .map_err(|_|anyhow!("{}",value["pick_error"].as_str().unwrap_or("Capture-matched camera geometry unavailable")))?;
    ensure!(geometry.frame==frame,"Camera is aligned to another Home scene");
    let direction=if let Some(pixel)=pixel {
        let mut picks=crate::picking::Picks::default();
        let token=picks.remember_camera(value["picking"].clone())?;
        let crate::picking::PickRequest::Scene{direction,..}=picks.camera_ray(&token,pixel)? else {unreachable!()};direction
    }else{
        let point=crate::local_mapper::compose(&geometry.camera_to_map,&auki_odometry::Pose{translation:[0.,0.,1.],..crate::local_mapper::identity()})?;
        std::array::from_fn(|i|point.translation[i]-geometry.camera_to_map.translation[i])
    };
    Ok(json!({"origin":geometry.camera_to_map.translation,"direction":direction,"frame":frame,"capture_timestamp_ns":value["capture_timestamp_ns"],"clock":value["clock"]}))
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn rays_use_calibrated_optical_pose_and_reject_other_scene(){
        let v=json!({"picking":{"frame":"home","camera_to_map":{"translation":[1,2,3],"rotation_xyzw":[0,0,0,1]},"calibration":{"width":100,"height":80,"K":[100,0,50,0,100,40,0,0,1],"D":[],"distortion_model":"none"}}});
        assert_eq!(ray(&v,None,"home").unwrap()["direction"],json!([0.,0.,1.]));
        assert_eq!(ray(&v,Some([50.,40.]),"home").unwrap()["direction"],json!([0.,0.,1.]));
        assert!(ray(&v,None,"other").is_err());assert!(ray(&v,Some([-1.,0.]),"home").is_err());
    }
}
