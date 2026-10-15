//! Deterministic, typed adapters to the same scene tools used by chat.
//! No model calls, inferred identities, robot-specific dispatch or retries.
use anyhow::{bail, ensure, Result};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn definitions() -> Value {
    json!([
        {"name":"camera","description":"Open one peer camera in PiP","args":[{"name":"peer","kind":"peer"},{"name":"camera","kind":"camera"}]},
        {"name":"attach","description":"Follow a peer camera in the 3D viewer","args":[{"name":"peer","kind":"peer"},{"name":"camera","kind":"camera"}]},
        {"name":"detach","description":"Stop following; restore manual view control","args":[]},
        {"name":"focus","description":"Move the view to a peer or scene entity","args":[{"name":"target","kind":"target"}]},
        {"name":"path","description":"Draw a floor route through peers or entities in order","args":[{"name":"waypoint","kind":"target","variadic":true,"min":2,"max":64}]},
        {"name":"inspect","description":"Read a peer Catalog or scene entity","args":[{"name":"target","kind":"target"}]},
        {"name":"rename","description":"Set a peer alias or rename a note/volume","args":[{"name":"target","kind":"editable_target"},{"name":"name","kind":"text"}]},
        {"name":"move","description":"Move a note or volume (XYZ metres, selected scene)","args":[{"name":"entity","kind":"editable_entity"},{"name":"x","kind":"number"},{"name":"y","kind":"number"},{"name":"z","kind":"number"}]},
        {"name":"resize","description":"Resize a volume (width, height, depth in metres)","args":[{"name":"volume","kind":"volume"},{"name":"width","kind":"number"},{"name":"height","kind":"number"},{"name":"depth","kind":"number"}]},
        {"name":"note","description":"Create a note (XYZ metres, selected scene)","args":[{"name":"name","kind":"text"},{"name":"x","kind":"number"},{"name":"y","kind":"number"},{"name":"z","kind":"number"}]}
    ])
}

/// A small projection of existing scene/Catalog data, not a second registry.
pub fn inventory(scene: Value, network: &Value) -> Result<Value> {
    let mut entities=Vec::new();
    for note in scene["notes"].as_array().into_iter().flatten() {
        if note["authored"]==true {entities.push(json!({"kind":"note","id":note["id"],"name":note["text"]}));}
    }
    for v in scene["semantic_volumes"]["items"].as_array().into_iter().flatten() {
        entities.push(json!({"kind":"volume","id":v["id"],"name":v["title"]}));
    }
    for p in scene["anchors"].as_array().into_iter().flatten() {
        if p["pose_in_map"]["translation"].is_array() {
            entities.push(json!({"kind":"portal","id":p["anchor_id"],"name":p["payload"].as_str().or_else(||p["anchor_id"].as_str()),"position":p["pose_in_map"]["translation"]}));
        }
    }
    let catalogs=network["peers"].as_array().into_iter().flatten().filter_map(|p|p["peer_id"].as_str().map(|id|(id.to_owned(),p.clone()))).collect();
    let cameras=crate::sensor_preview::camera_sensors(&catalogs);
    let peers=network["peers"].as_array().into_iter().flatten().map(|p| {
        let id=p["peer_id"].as_str().unwrap_or("");
        let name=p["alias"].as_str().or_else(||network["aliases"][id].as_str()).or_else(||p["name"].as_str()).unwrap_or(id);
        let choices=cameras.iter().filter(|c|c.reference.peer_id==id).map(|c|json!({
            "product_id":c.reference.product_id,"reference":c.reference,
            "frame_id":c.metadata.as_ref().map(|m|m.value["frame"]["id"].clone())
        })).collect::<Vec<_>>();
        json!({"id":id,"name":name,"status":p["status"],"cameras":choices})
    }).collect::<Vec<_>>();
    let value=json!({"scene":scene["scene"],"frame":scene["frame"],"commands":definitions(),"peers":peers,"entities":entities});
    ensure!(serde_json::to_vec(&value)?.len()<=512*1024,"Command inventory exceeds limits");
    Ok(value)
}

#[derive(Debug, Deserialize)]
#[serde(tag="type",rename_all="snake_case",deny_unknown_fields)]
pub enum Argument {
    Peer {id:String},
    Entity {kind:String,id:String},
    Camera {reference:Value},
    Text {value:String},
    Number {value:f64},
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub scene:String,pub frame:String,pub command:String,pub args:Vec<Argument>,
    pub viewer:Option<crate::chatbot::Viewer>,
}

fn entity<'a>(arg:&Argument,inventory:&'a Value)->Result<&'a Value> {
    let Argument::Entity{kind,id}=arg else {bail!("Select an @entity from autocomplete")};
    inventory["entities"].as_array().into_iter().flatten().find(|e|e["id"]==*id&&e["kind"]==*kind)
        .ok_or_else(||anyhow::anyhow!("Entity no longer exists in the selected scene; select it again"))
}
fn peer<'a>(arg:&Argument,inventory:&'a Value)->Result<&'a Value> {
    let Argument::Peer{id}=arg else {bail!("Select a #peer from autocomplete")};
    inventory["peers"].as_array().into_iter().flatten().find(|p|p["id"]==*id)
        .ok_or_else(||anyhow::anyhow!("Peer is no longer discovered; select it again"))
}
fn text(arg:&Argument)->Result<&str> {
    let Argument::Text{value}=arg else {bail!("Expected text; quote names containing spaces")};
    ensure!(!value.trim().is_empty()&&value.len()<=256,"Name must be 1–256 bytes");Ok(value)
}
fn vector(args:&[Argument])->Result<[f64;3]> {
    ensure!(args.len()==3,"Expected three numbers in metres");
    let mut p=[0.;3];
    for (i,a) in args.iter().enumerate(){let Argument::Number{value}=a else {bail!("Expected numeric XYZ arguments")};ensure!(value.is_finite()&&value.abs()<=10_000.,"Invalid coordinate or dimension");p[i]=*value;}
    Ok(p)
}

/// Resolve fresh data, then execute existing handlers, with at most one write.
pub async fn execute<F:std::future::Future<Output=Result<Value>>>(request:&Request,inventory:&Value,mut tool:impl FnMut(String,Value)->F)->Result<Value> {
    ensure!(inventory["scene"]==request.scene&&inventory["frame"]==request.frame&&!request.frame.is_empty(),"Selected scene/frame changed; select your references again");
    ensure!(request.viewer.as_ref().is_some_and(|v|v.frame==request.frame),"Viewer frame changed; refresh Home");
    let defs=definitions();
    let def=defs.as_array().unwrap().iter().find(|d|d["name"]==request.command)
        .ok_or_else(||anyhow::anyhow!(if request.command=="point"{"Physical pointing is not exposed by Home's tool handlers; no robot command was sent"}else{"Unknown slash command; no model was invoked"}))?;
    if request.command=="path" {ensure!((2..=64).contains(&request.args.len()),"/path needs 2–64 waypoints");}
    else {ensure!(request.args.len()==def["args"].as_array().unwrap().len(),"Wrong number of arguments for /{}",request.command);}
    let a=&request.args;
    let mut changed=false;
    let mut ui=false;
    let value=match request.command.as_str() {
        "path"=>{
            let mut targets=Vec::new();
            for arg in a {
                let is_peer=matches!(arg,Argument::Peer{..});
                let target=if is_peer {peer(arg,inventory)?}else{entity(arg,inventory)?};
                targets.push(json!({"kind":if is_peer {"peer"}else{target["kind"].as_str().unwrap_or("")},"id":target["id"]}));
            }
            changed=true;tool("create_path".into(),json!({"waypoints":targets})).await?
        },
        "detach"=>{ui=true;tool("detach_camera".into(),json!({})).await?},
        "camera"|"attach"=>{
            let p=peer(&a[0],inventory)?;
            let Argument::Camera{reference}=&a[1] else {bail!("Select a camera from autocomplete")};
            let camera=p["cameras"].as_array().into_iter().flatten().find(|c|c["reference"]==*reference)
                .ok_or_else(||anyhow::anyhow!("Camera declaration changed or disappeared; select it again"))?;
            ui=true;
            if request.command=="camera" {tool("open_camera_pip".into(),json!({"peer_id":p["id"],"product_id":camera["product_id"]})).await?}
            else {ensure!(camera["frame_id"].is_string(),"Camera lacks a registered frame");tool("attach_camera".into(),json!({"peer_id":p["id"],"product_id":camera["product_id"],"frame_id":camera["frame_id"]})).await?}
        },
        "note"=>{let name=text(&a[0])?;let position=vector(&a[1..])?;changed=true;tool("create_note".into(),json!({"name":name,"description":"","position":position})).await?},
        operation=>{
            let is_peer=matches!(&a[0],Argument::Peer{..});
            let target=if is_peer {peer(&a[0],inventory)?}else{entity(&a[0],inventory)?};
            let id=&target["id"];
            let kind=if is_peer {"peer"}else{target["kind"].as_str().unwrap_or("")};
            if operation=="rename"&&is_peer {
                changed=true;tool("set_peer_alias".into(),json!({"peer_id":id,"alias":text(&a[1])?})).await?
            }else if operation=="inspect"&&is_peer {
                tool("inspect_catalog".into(),json!({"peer_id":id,"section":"products","limit":10})).await?
            }else{
                ensure!(operation=="focus"||operation=="inspect"||matches!(kind,"note"|"volume"),"This entity is read-only");
                ensure!(operation!="resize"||kind=="volume","Only volumes can be resized");
                let object=match kind {
                    "note"=>tool("get_note".into(),json!({"id":id})).await?,
                    "volume"=>tool("get_semantic_volume".into(),json!({"id":id})).await?,
                    "peer"=>tool("get_peer_pose".into(),json!({"peer_id":id})).await?,
                    "portal"=>target.clone(),
                    _=>bail!("Unsupported entity kind"),
                };
                match operation {
                    "inspect"=>object,
                    "focus"=>{
                        let center=if kind=="volume"{&object["center"]}else if kind=="peer"{&object["location"]["position"]}else{&object["position"]};
                        let center:[f64;3]=serde_json::from_value(center.clone())?;
                        let radius=if kind=="volume" {let size:[f64;3]=serde_json::from_value(object["size"].clone())?;size.iter().map(|x|x*x).sum::<f64>().sqrt()*1.5}else{3.};
                        let distance=radius.max(3.);ensure!(distance<=60.,"Object is too large to frame within the viewer's range");
                        let position=[center[0]+distance*0.6,center[1]+distance*0.5,center[2]+distance*0.6];
                        ui=true;tool("move_camera".into(),json!({"position":position,"look_at":center})).await?
                    },
                    "rename"=>{
                        let name=text(&a[1])?;changed=true;
                        tool(format!("rename_{kind}"),json!({"id":id,"expected_name":if kind=="note"{&object["name"]}else{&object["title"]},"name":name})).await?
                    },
                    "move"=>{
                        let position=vector(&a[1..])?;changed=true;
                        if kind=="note"{tool("move_note".into(),json!({"id":id,"expected_position":object["position"],"position":position})).await?}
                        else{tool("move_volume".into(),json!({"id":id,"expected_center":object["center"],"center":position})).await?}
                    },
                    "resize"=>{let size=vector(&a[1..])?;ensure!(size.iter().all(|n|*n>0.),"Dimensions must be positive");changed=true;tool("resize_volume".into(),json!({"id":id,"expected_size":object["size"],"size":size})).await?},
                    _=>bail!("Unknown command"),
                }
            }
        }
    };
    let reply=if ui {"View change requested.".into()}else if changed {value["summary"].as_str().unwrap_or("Change saved by Home.").to_owned()}
        else {serde_json::to_string_pretty(&value)?};
    Ok(json!({"reply":reply,"source":"command","changes":if changed{vec![value.clone()]}else{vec![]},"ui_actions":if ui{vec![value]}else{vec![]}}))
}
