//! Read-only calculations. Frame bindings and the captured camera are supplied by Home.
use crate::{chatbot::Viewer, semantic_volumes::Volume};
use anyhow::{Result, ensure};
use auki_datatypes::pose::{Quat, SpatialTransform, Vec3};
use serde::Deserialize;
use serde_json::{Value, json};
fn transform(v: &Volume) -> SpatialTransform {
    SpatialTransform {
        translation: Some(Vec3 {
            x: v.center[0],
            y: 0.,
            z: v.center[2],
        }),
        orientation: Some(Quat {
            x: 0.,
            y: (v.yaw / 2.).sin(),
            z: 0.,
            w: (v.yaw / 2.).cos(),
        }),
    }
}
fn point(t: &SpatialTransform, p: [f64; 3]) -> Result<[f64; 3]> {
    let local = SpatialTransform {
        translation: Some(Vec3 {
            x: p[0],
            y: p[1],
            z: p[2],
        }),
        orientation: None,
    };
    let result = auki_geometry::compose_spatial_transforms(&local, t)?
        .translation
        .unwrap();
    Ok([result.x, result.y, result.z])
}
fn camera(view: &Viewer) -> Result<[f64; 3]> {
    let p = view
        .camera_position
        .ok_or_else(|| anyhow::anyhow!("Captured camera missing; refresh Home"))?;
    ensure!(
        view.mode == "3d"
            && p.iter()
                .chain(view.screen_right.iter())
                .all(|n| n.is_finite()),
        "Valid 3D viewpoint required"
    );
    Ok(p)
}
fn corners(v: &Volume) -> Result<Vec<(String, [f64; 3], [f64; 3])>> {
    ensure!(
        v.center.iter().chain(v.size.iter()).all(|n| n.is_finite())
            && v.size.iter().all(|n| *n > 0.)
            && v.yaw.is_finite(),
        "Invalid volume geometry"
    );
    let mut out = vec![];
    for x in [-1., 1.] {
        for z in [-1., 1.] {
            let local = [x * v.size[0] / 2., 0., z * v.size[2] / 2.];
            out.push((
                format!(
                    "x{}:z{}",
                    if x < 0. { "-" } else { "+" },
                    if z < 0. { "-" } else { "+" }
                ),
                point(&transform(v), local)?,
                local,
            ));
        }
    }
    Ok(out)
}
fn selected(v: &Volume, view: &Viewer, selector: &str) -> Result<(String, [f64; 3], [f64; 3])> {
    let camera = camera(view)?;
    let mut points = corners(v)?;
    if selector.starts_with('x') {
        return points
            .into_iter()
            .find(|p| p.0 == selector)
            .ok_or_else(|| anyhow::anyhow!("Unknown corner ID"));
    }
    ensure!(
        matches!(selector, "far" | "near" | "left" | "right"),
        "Unknown corner selector"
    );
    if matches!(selector, "left" | "right") {
        ensure!(
            view.screen_right[0].hypot(view.screen_right[2]) > 1e-6,
            "Viewer has no horizontal right direction"
        );
    }
    let score = |p: &(String, [f64; 3], [f64; 3])| {
        let distance = (p.1[0] - camera[0]).hypot(p.1[2] - camera[2]);
        let primary = match selector {
            "near" => -distance,
            "far" => distance,
            "left" => {
                -((p.1[0] - v.center[0]) * view.screen_right[0]
                    + (p.1[2] - v.center[2]) * view.screen_right[2])
            }
            _ => {
                (p.1[0] - v.center[0]) * view.screen_right[0]
                    + (p.1[2] - v.center[2]) * view.screen_right[2]
            }
        };
        (primary, distance)
    };
    points.sort_by(|a, b| {
        let a = score(a);
        let b = score(b);
        a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
    });
    Ok(points.pop().unwrap())
}
pub fn get_volume_corners(v: &Volume, view: &Viewer, frame: &str) -> Result<Value> {
    let mut labels = serde_json::Map::new();
    for label in ["far", "near", "left", "right"] {
        labels.insert(label.into(), json!(selected(v, view, label)?.0));
    }
    Ok(
        json!({"frame":frame,"ground_y":0,"volume_id":v.id,"corners":corners(v)?.iter().map(|p|json!({"id":p.0,"position":p.1})).collect::<Vec<_>>(),"labels":labels,"side_tie_break":"farther from camera"}),
    )
}
pub fn point_inside_volume(
    v: &Volume,
    view: &Viewer,
    selector: &str,
    distance: f64,
    mode: &str,
    frame: &str,
) -> Result<Value> {
    ensure!(
        distance.is_finite() && distance >= 0.,
        "Invalid inset distance"
    );
    let (id, _, mut p) = selected(v, view, selector)?;
    match mode {
        "toward_center" => {
            let length = p[0].hypot(p[2]);
            ensure!(
                distance <= length,
                "Inset exceeds distance to volume center"
            );
            if length > 0. {
                let scale = 1. - distance / length;
                p[0] *= scale;
                p[2] *= scale;
            }
        }
        "from_walls" => {
            ensure!(
                distance <= v.size[0] / 2. && distance <= v.size[2] / 2.,
                "Inset exceeds volume half-width/depth"
            );
            p[0] -= p[0].signum() * distance;
            p[2] -= p[2].signum() * distance;
        }
        _ => anyhow::bail!("Unknown inset mode"),
    }
    Ok(
        json!({"frame":frame,"volume_id":v.id,"corner_id":id,"position":point(&transform(v),p)?,"ground_y":0}),
    )
}
pub fn offset_point(
    mut position: [f64; 3],
    view: &Viewer,
    direction: &str,
    distance: f64,
    basis: &str,
    ground: bool,
    frame: &str,
) -> Result<Value> {
    ensure!(
        position.iter().all(|n| n.is_finite())
            && distance.is_finite()
            && distance >= 0.
            && distance <= 10000.,
        "Invalid offset"
    );
    ensure!(matches!(basis, "viewer" | "scene"), "Invalid basis");
    if ground {
        position[1] = 0.;
    }
    let mut delta = match direction {
        "toward_viewer" | "away_from_viewer" => {
            let p = camera(view)?;
            let sign = if direction == "toward_viewer" {
                1.
            } else {
                -1.
            };
            [
                (p[0] - position[0]) * sign,
                (p[1] - position[1]) * sign,
                (p[2] - position[2]) * sign,
            ]
        }
        "left" | "right" | "forward" | "backward" | "up" | "down" => {
            let local = match direction {
                "left" => [-1., 0., 0.],
                "right" => [1., 0., 0.],
                "forward" => [0., 0., -1.],
                "backward" => [0., 0., 1.],
                "up" => [0., 1., 0.],
                _ => [0., -1., 0.],
            };
            if basis == "scene" || matches!(direction, "up" | "down") {
                local
            } else {
                camera(view)?;
                let yaw = view.screen_right[2].atan2(view.screen_right[0]);
                let t = SpatialTransform {
                    translation: None,
                    orientation: Some(Quat {
                        x: 0.,
                        y: -(yaw / 2.).sin(),
                        z: 0.,
                        w: (yaw / 2.).cos(),
                    }),
                };
                point(&t, local)?
            }
        }
        _ => anyhow::bail!("Unknown offset direction"),
    };
    if ground {
        delta[1] = 0.;
    }
    let length = delta.iter().map(|n| n * n).sum::<f64>().sqrt();
    ensure!(
        length > 1e-9 || distance == 0.,
        "No direction on ground plane"
    );
    let distance = if direction == "toward_viewer" {
        distance.min(length)
    } else {
        distance
    };
    let translation = if length > 0. {
        delta.map(|n| n * distance / length)
    } else {
        [0.; 3]
    };
    let t = SpatialTransform {
        translation: Some(Vec3 {
            x: translation[0],
            y: translation[1],
            z: translation[2],
        }),
        orientation: None,
    };
    Ok(json!({"frame":frame,"position":point(&t,position)?,"ground":ground}))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corners {
    pub volume_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inside {
    pub volume_id: String,
    pub corner: String,
    pub distance_m: f64,
    pub mode: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offset {
    pub position: [f64; 3],
    pub direction: String,
    pub distance_m: f64,
    pub frame: String,
    pub ground: bool,
}
pub fn tools() -> Vec<Value> {
    let mut tools:Vec<Value>=serde_json::from_str(include_str!("../config/scene-geometry-tools.json"))
        .expect("validated geometry schemas");
    tools.push(json!({"type":"function","function":{"name":"raycast_peer_camera","description":"Read-only: raycast from any discovered peer camera using capture-matched registry calibration and transforms against occupied voxels or Y=0 floor, nearest hit within 40m. Optional pixel=[x,y], top-left origin in reported camera dimensions. Omit pixel for optical axis. Use returned position with create_note/move_note; no hit means no placement.","parameters":{"type":"object","properties":{"peer_id":{"type":"string"},"product_id":{"type":"string","description":"Exact advertised camera Product; choose best matching camera from Catalog."},"pixel":{"type":"array","items":{"type":"number"},"minItems":2,"maxItems":2}},"required":["peer_id"],"additionalProperties":false}}}));
    tools.push(json!({"type":"function","function":{"name":"point_relative_to_peer","description":"Read-only: offset in CURRENT peer orientation, not viewer. Resolve alias to peer_id. Set elevation_y=0 for ground/Y=0; otherwise preserves elevation. Use returned position with move_note/create_note. Missing orientation fails.","parameters":{"type":"object","properties":{"peer_id":{"type":"string"},"direction":{"type":"string","enum":["in_front","behind","left","right"]},"distance_m":{"type":"number","minimum":0},"elevation_y":{"type":"number","description":"Optional absolute scene Y in meters; ground is 0."}},"required":["peer_id","direction","distance_m"],"additionalProperties":false}}}));tools
}
/// Resolve a current peer pose, not its billboard (robot labels are above its base).
pub fn peer_relative_point(view:&Value,peer:&str,direction:&str,distance:f64,frame:&str)->Result<[f64;3]>{
 ensure!(distance.is_finite()&&distance>=0.&&distance<=1000.,"Invalid peer offset distance");
 let phone_id=format!("phone-name:{peer}");
 let phone=view["notes"].as_array().and_then(|notes|notes.iter().find(|n|n["id"]==phone_id));
 let (pose,local)=if let Some(phone)=phone {
  ensure!(phone["phone_pose"]["state"]!="asleep","Peer is asleep; current orientation unavailable");
  (&phone["phone_pose"]["pose"],match direction{"in_front"=>[0.,0.,-1.],"behind"=>[0.,0.,1.],"left"=>[-1.,0.,0.],"right"=>[1.,0.,0.],_=>anyhow::bail!("Invalid peer direction")})
 }else{
  let robot_id=format!("robot-name:{peer}");
  ensure!(view["notes"].as_array().is_some_and(|notes|notes.iter().any(|n|n["id"]==robot_id)),"Peer has no current localized pose");
  let robot=&view["local_mapping"]["runners"][peer]["robot_pose"];
  let robot=if robot.is_null(){let candidate=&view["local_mapping"]["robot_pose"];ensure!(candidate["clock"]["peer_id"]==peer,"Peer has no current localized pose");candidate}else{robot};
  ensure!(robot["frame"]==frame,"Peer pose belongs to another scene");
  // glyph_pose explicitly converts the declared robot body to ROS axes:
  // +X forward, +Y left. Never assume body axes from the map convention.
  (&robot["glyph_pose"],match direction{"in_front"=>[1.,0.,0.],"behind"=>[-1.,0.,0.],"left"=>[0.,1.,0.],"right"=>[0.,-1.,0.],_=>anyhow::bail!("Invalid peer direction")})
 };
 ensure!(pose["to_frame_id"]==frame,"Peer orientation unavailable in selected scene");
 let position:[f64;3]=serde_json::from_value(pose["translation"].clone())?;
 let q:[f64;4]=serde_json::from_value(pose["rotation_wxyz"].clone()).map_err(|_|anyhow::anyhow!("Peer orientation unavailable"))?;
 ensure!(position.iter().chain(q.iter()).all(|n|n.is_finite())&&(q.iter().map(|n|n*n).sum::<f64>()-1.).abs()<1e-3,"Invalid peer pose");
 let rotation=SpatialTransform{translation:None,orientation:Some(Quat{w:q[0],x:q[1],y:q[2],z:q[3]})};
 let mut delta=point(&rotation,local)?;delta[1]=0.;
 let length=delta[0].hypot(delta[2]);ensure!(length>1e-6,"Peer direction has no ground-plane projection");
 Ok([position[0]+delta[0]*distance/length,position[1],position[2]+delta[2]*distance/length])
}
/// An explicit user Y constraint takes precedence over inherited peer elevation.
pub fn placement_elevation(mut position:[f64;3],elevation_y:Option<f64>,instruction:&str)->Result<[f64;3]>{
 let compact=instruction.to_lowercase().chars().filter(|c|!c.is_whitespace()).collect::<String>();
 let explicit=compact.split_once("y=").and_then(|(_,tail)|{
  let number=tail.chars().take_while(|c|c.is_ascii_digit()||matches!(c,'-'|'+'|'.')).collect::<String>();
  number.parse::<f64>().ok()
 });
 let ground=["on the ground","at ground level","on the floor"].iter().any(|s|instruction.to_lowercase().contains(s));
 if let Some(y)=explicit.or(if ground{Some(0.)}else{elevation_y}) {ensure!(y.is_finite(),"Elevation must be finite");position[1]=y;}
 Ok(position)
}
pub fn peer_camera_ray(view:&Value,peer:&str,pixel:Option<[f64;2]>,frame:&str,calibration:Option<&Value>)->Result<Value>{
 let id=format!("phone-name:{peer}");
 let camera=view["notes"].as_array().and_then(|notes|notes.iter().find(|n|n["id"]==id)).map(|n|&n["phone_pose"]).ok_or_else(||anyhow::anyhow!("Peer has no current localized camera pose"))?;
 ensure!(camera["state"]=="live"&&camera["received_age_ms"].as_u64().is_some_and(|age|age<3000),"Camera pose is stale or asleep");
 let pose:auki_scenegraph::RigidTransform=serde_json::from_value(camera["pose"].clone())?;
 ensure!(pose.to_frame_id==frame&&pose.translation.iter().chain(pose.rotation_wxyz.iter()).all(|n|n.is_finite())&&(pose.rotation_wxyz.iter().map(|n|n*n).sum::<f64>()-1.).abs()<1e-3,"Invalid camera pose or frame");
 let local=if let Some(pixel)=pixel {
  let k=calibration.ok_or_else(||anyhow::anyhow!("Camera registry calibration unavailable"))?;
  let width=k["width"].as_u64().filter(|n|*n>0&&*n<=8192).ok_or_else(||anyhow::anyhow!("Invalid camera width"))?;
  let height=k["height"].as_u64().filter(|n|*n>0&&*n<=8192).ok_or_else(||anyhow::anyhow!("Invalid camera height"))?;
  ensure!(pixel.iter().all(|x|x.is_finite())&&(0. ..width as f64).contains(&pixel[0])&&(0. ..height as f64).contains(&pixel[1]),"Pixel outside camera image");
  let optical=crate::ar_preview::pixel_ray(k,pixel)?;
  // Phone AR poses use OpenGL camera axes; pixel unprojection is OpenCV.
  [optical[0],-optical[1],-optical[2]]
 }else{[0.,0.,-1.]};
 let [w,x,y,z]=pose.rotation_wxyz;
 let direction=point(&SpatialTransform{translation:None,orientation:Some(Quat{w,x,y,z})},local)?;
 Ok(json!({"origin":pose.translation,"direction":direction,"frame":frame,"capture_timestamp_ns":camera["capture_timestamp_ns"],"camera_configuration_id":camera["camera_configuration_id"],"calibration":calibration}))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraRay {pub peer_id:String,#[serde(default)]pub pixel:Option<[f64;2]>}
/// Snap model-copied coordinates to the authoritative query result. One micron
/// tolerates JSON/decimal round trips without accepting a guessed destination.
pub fn resolve_calculated_point(requested:[f64;3],points:&[[f64;3]])->Result<[f64;3]>{
 ensure!(requested.iter().all(|n|n.is_finite()),"Invalid requested position");
 points.iter().find(|p|p.iter().zip(requested).all(|(a,b)|(*a-b).abs()<=1e-6)).copied().ok_or_else(||anyhow::anyhow!("Call a read-only geometry tool and use its returned position; do not guess coordinates"))
}
