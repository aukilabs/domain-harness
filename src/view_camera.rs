//! Browser-local map viewing, never a device operation or a saved scene edit.
use anyhow::{ensure, Result};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn tool() -> Value {
    json!({"type":"function","function":{"name":"move_camera",
        "description":"Move this user's Home 3D map-view camera, NOT a physical camera or robot. Specify its position and look_at point in the selected scene's XYZ meters, Y up (upright, no roll). Use current viewer context or inspect scene objects to choose the view. Position-to-target distance must be 0.01 to 60 meters. Switches the browser to 3D; does not edit the scene or open a camera feed.",
        "parameters":{"type":"object","properties":{
            "position":{"type":"array","items":{"type":"number"},"minItems":3,"maxItems":3},
            "look_at":{"type":"array","items":{"type":"number"},"minItems":3,"maxItems":3}
        },"required":["position","look_at"],"additionalProperties":false}}})
}

/// Same fresh, calibrated presence used by the map's phone overlay. A transport
/// connection or sleeping/expired pose is not a current location.
pub fn peer_position(presence:&Value,peer:&str,frame:&str)->Option<Value>{
 if presence["frame"]!=frame{return None;}
 let pose=presence["poses"].as_array()?.iter().find(|p|p["peer_id"]==peer&&p["state"]=="live"&&p["received_age_ms"].as_u64().is_some_and(|age|age<3000))?;
 let position:[f64;3]=serde_json::from_value(pose["pose"]["translation"].clone()).ok()?;
 if !position.iter().all(|n|n.is_finite()){return None;}
 Some(json!({"position":position,"frame":frame,"received_age_ms":pose["received_age_ms"],"source":"live calibrated presence"}))
}

pub fn attach_tools() -> Vec<Value> {
    vec![
        json!({"type":"function","function":{"name":"inspect_sensor_frames","description":"List a discovered peer's typed sensor pose frames and exact Product IDs. Resolve peer aliases through inspect_catalog first. Metadata is not proof of live tracking. No video streams are opened.","parameters":{"type":"object","properties":{"peer_id":{"type":"string"}},"required":["peer_id"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"attach_camera","description":"Continuously attach this browser's 3D map camera to a peer sensor frame (for example see the world from Dan's Phone). Resolve exact peer_id and frame_id with inspect_sensor_frames, choosing the best matching sensor. Include product_id if multiple pose products cover that frame. Requires a valid live transform into the selected Home scene; pauses on stale/lost tracking. This is a virtual viewpoint, NOT a video feed or hardware movement.","parameters":{"type":"object","properties":{"peer_id":{"type":"string"},"frame_id":{"type":"string"},"product_id":{"type":"string"}},"required":["peer_id","frame_id"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"detach_camera","description":"Stop following a peer sensor and restore manual control of this browser's map camera at its current location. No device or scene edits.","parameters":{"type":"object","properties":{},"additionalProperties":false}}})
    ]
}

fn frames(network:&Value)->Vec<Value> {
    let catalogs=network["peers"].as_array().into_iter().flatten().filter_map(|p|p["peer_id"].as_str().map(|id|(id.to_string(),p.clone()))).collect();
    crate::sensor_preview::frame_sensors(&catalogs).iter().map(|s|json!({
        "peer_id":s.reference.peer_id,"frame_id":s.metadata.as_ref().unwrap().value["from_frame"]["frame_id"],
        "pose_product":s.reference,"sensor_id":s.id,"source_frame":s.metadata.as_ref().unwrap().value["from_frame"],
        "to_frame":s.metadata.as_ref().unwrap().value["to_frame"],"alias":network["aliases"][&s.reference.peer_id]
    })).collect()
}

pub fn inspect_frames(network:&Value,args:Value)->Result<Value> {
    #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Query {peer_id:String}
    let q:Query=serde_json::from_value(args)?;
    Ok(json!({"frames":frames(network).into_iter().filter(|f|f["peer_id"]==q.peer_id).collect::<Vec<_>>()}))
}

pub fn attach_action(network:&Value,args:Value,scene:&str,frame:&str,viewer:Option<&crate::chatbot::Viewer>)->Result<Value> {
    #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Attach {peer_id:String,frame_id:String,product_id:Option<String>}
    let q:Attach=serde_json::from_value(args)?;
    ensure!(!frame.is_empty() && viewer.is_some_and(|v|v.frame==frame),"Viewer frame changed; refresh Home");
    let choices:Vec<_>=frames(network).into_iter().filter(|f|f["peer_id"]==q.peer_id && f["frame_id"]==q.frame_id
        && q.product_id.as_ref().is_none_or(|p|f["pose_product"]["product_id"]==*p)).collect();
    ensure!(choices.len()==1,"No unique typed sensor pose: inspect_sensor_frames and choose an exact frame and product");
    let mut action=choices[0].clone();action["type"]=json!("attach_camera");action["scene"]=json!(scene);action["frame"]=json!(frame);
    Ok(action)
}

/// Transform the declared sensor-forward and sensor-up axes, preserving roll.
/// Never assume a phone optical frame has the same axes as a robot body frame.
pub fn frame_view(pose:&auki_odometry::Pose,frame:&auki_registry::FrameRegistryEntry)->Result<Value> {
    use auki_registry::AxisDirection as D;
    pose.validate()?;frame.validate().map_err(anyhow::Error::msg)?;
    ensure!(frame.handedness==auki_registry::Handedness::Right,"Unsupported left-handed viewpoint frame");
    ensure!(frame.units==auki_registry::LengthUnit::Meters,"Viewpoint frame must use meters");
    let axes=[frame.axes.x,frame.axes.y,frame.axes.z];
    let direction=|positive,negative| -> Result<[f64;3]> {
        let vector=axes.map(|d|if d==positive {1.}else if d==negative {-1.}else{0.});
        let transformed=crate::local_mapper::compose(pose,&auki_odometry::Pose{translation:vector,..crate::local_mapper::identity()})?;
        Ok(std::array::from_fn(|i|transformed.translation[i]-pose.translation[i]))
    };
    let forward=direction(D::Forward,D::Backward)?;
    Ok(json!({"position":pose.translation,"look_at":std::array::from_fn::<_,3,_>(|i|pose.translation[i]+forward[i]),"up":direction(D::Up,D::Down)?}))
}

pub fn action(args: Value, scene: &str, frame: &str, viewer: Option<&crate::chatbot::Viewer>) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Pose { position: [f64; 3], look_at: [f64; 3] }
    let p: Pose = serde_json::from_value(args)?;
    let viewer = viewer.ok_or_else(|| anyhow::anyhow!("Viewer context missing; refresh Home"))?;
    ensure!(!frame.is_empty() && viewer.frame == frame, "Viewer frame changed; retry in the current scene");
    ensure!(p.position.iter().chain(p.look_at.iter()).all(|x| x.is_finite() && x.abs() <= 10_000.), "Invalid camera coordinates");
    let distance = p.position.iter().zip(p.look_at).map(|(a,b)| (a-b).powi(2)).sum::<f64>().sqrt();
    ensure!((0.01..=60.).contains(&distance), "Camera target must be 0.01 to 60 meters from its position");
    Ok(json!({"type":"move_camera","scene":scene,"frame":frame,"position":p.position,"look_at":p.look_at}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn peer_location_requires_fresh_live_matching_frame(){
        let mut p=json!({"frame":"home","poses":[{"peer_id":"phone","state":"live","received_age_ms":100,"pose":{"translation":[2,1,3]}}]});
        assert_eq!(peer_position(&p,"phone","home").unwrap()["position"],json!([2.,1.,3.]));
        assert!(peer_position(&p,"phone","elsewhere").is_none());
        p["poses"][0]["state"]=json!("asleep");assert!(peer_position(&p,"phone","home").is_none());
        p["poses"][0]["state"]=json!("live");p["poses"][0]["received_age_ms"]=json!(3000);assert!(peer_position(&p,"phone","home").is_none());
    }
    #[test]
    fn follows_declared_optical_body_and_opengl_axes_with_roll() -> Result<()> {
        let pose=crate::local_mapper::identity();
        for (frame,forward,up) in [
            (auki_registry::FrameRegistryEntry::ros_optical("peer","camera"),[0.,0.,1.],[0.,-1.,0.]),
            (auki_registry::FrameRegistryEntry::opengl("peer","phone"),[0.,0.,-1.],[0.,1.,0.]),
            (auki_registry::FrameRegistryEntry::ros_body("peer","lidar"),[1.,0.,0.],[0.,0.,1.])
        ] {
            let v=frame_view(&pose,&frame)?;
            assert_eq!(serde_json::from_value::<[f64;3]>(v["look_at"].clone())?,forward);
            assert_eq!(serde_json::from_value::<[f64;3]>(v["up"].clone())?,up);
        }
        let pose=auki_odometry::Pose{translation:[1.,2.,3.],rotation_xyzw:[0.,0.,std::f64::consts::FRAC_1_SQRT_2,std::f64::consts::FRAC_1_SQRT_2]};
        let v=frame_view(&pose,&auki_registry::FrameRegistryEntry::opengl("peer","phone"))?;
        assert!((v["up"][0].as_f64().unwrap()+1.).abs()<1e-9);
        assert_eq!(v["position"],json!([1.,2.,3.]));Ok(())
    }
    #[test]
    fn camera_action_is_frame_bound_and_validated() {
        let viewer = serde_json::from_value(json!({"frame":"home","mode":"3d","screen_right":[1,0,0]})).unwrap();
        let args = json!({"position":[2,3,4],"look_at":[0,0,0]});
        let result = action(args.clone(), "local:home", "home", Some(&viewer)).unwrap();
        assert_eq!(result["scene"], "local:home");
        assert_eq!(result["type"], "move_camera");
        assert!(action(args.clone(), "local:home", "other", Some(&viewer)).is_err());
        assert!(action(args, "local:home", "home", None).is_err());
        for args in [json!({"position":[0,0,0],"look_at":[0,0,0]}),
            json!({"position":[0,0,61],"look_at":[0,0,0]}),
            json!({"position":[0,1,2],"look_at":[0,0,0],"peer_id":"robot"}),
            json!({"position":[0,1],"look_at":[0,0,0]})] {
            assert!(action(args, "local:home", "home", Some(&viewer)).is_err());
        }
    }
}
