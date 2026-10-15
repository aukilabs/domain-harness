use serde_json::{json,Value};
use spatial_home::commands::{execute,Request};

fn inventory()->Value {json!({"scene":"local:test","frame":"home","peers":[{"id":"phone","name":"Dan's phone","cameras":[{"product_id":"rgb","reference":{"peer_id":"phone","product_id":"rgb","hash":"current"},"frame_id":"optical"}]}],"entities":[{"kind":"note","id":"cheese","name":"cheese"},{"kind":"volume","id":"room","name":"room"},{"kind":"portal","id":"portal","name":"origin","position":[0,0,0]}]})}
fn request(command:&str,args:Value)->Request {serde_json::from_value(json!({"scene":"local:test","frame":"home","command":command,"args":args,"viewer":{"frame":"home","mode":"3d","screen_right":[1,0,0],"camera_position":[3,3,3],"camera_target":[0,0,0]}})).unwrap()}
fn numbers()->Vec<Value>{[1.,2.,3.].map(|n|json!({"type":"number","value":n})).to_vec()}

#[tokio::test]
async fn camera_uses_existing_handler_and_exact_product_without_model(){
 let r=request("camera",json!([{"type":"peer","id":"phone"},{"type":"camera","reference":inventory()["peers"][0]["cameras"][0]["reference"]}]));
 let mut calls=vec![];
 let result=execute(&r,&inventory(),|name,args|{
  calls.push(name.clone());assert_eq!(name,"open_camera_pip");
  std::future::ready(spatial_home::catalog_chat::select_camera(&[json!({"id":"camera-sensor","peer_id":"phone","product_id":"rgb"})],args))
 }).await.unwrap();
 assert_eq!(calls,vec!["open_camera_pip"]);assert_eq!(result["ui_actions"][0]["sensor_id"],"camera-sensor");assert_eq!(result["changes"],json!([]));
}

#[tokio::test]
async fn fresh_read_then_one_edit_preserves_expected_state(){
 let mut a=vec![json!({"type":"entity","kind":"note","id":"cheese"})];a.extend(numbers());
 let r=request("move",json!(a));let mut calls=vec![];
 let result=execute(&r,&inventory(),|name,args|{
  calls.push((name.clone(),args.clone()));
  std::future::ready(Ok(if name=="get_note"{json!({"id":"cheese","name":"Cheese","position":[4,5,6]})}else{json!({"summary":"Note moved","after":[1,2,3]})}))
 }).await.unwrap();
 assert_eq!(calls.len(),2);assert_eq!(calls[0].0,"get_note");assert_eq!(calls[1].0,"move_note");assert_eq!(calls[1].1["expected_position"],json!([4,5,6]));assert_eq!(result["changes"].as_array().unwrap().len(),1);
}

#[tokio::test]
async fn volume_command_uses_real_storage_validation(){
 use spatial_home::semantic_volumes::{Storage,Volume};
 let dir=tempfile::tempdir().unwrap();let store=Storage::new(dir.path().into());
 let id=uuid::Uuid::new_v4().to_string();let v=Volume{id:id.clone(),title:"Room".into(),description:"Keep me".into(),semantic_type:"room".into(),center:[0.,1.,0.],size:[2.,2.,2.],yaw:0.};
 store.change("test","home",None,Some(v),"home").unwrap();
 let mut inv=inventory();inv["entities"][1]["id"]=json!(id);
 let r=request("rename",json!([{"type":"entity","kind":"volume","id":id},{"type":"text","value":"Kitchen"}]));
 execute(&r,&inv,|name,args|std::future::ready(if name=="get_semantic_volume"{Ok(json!(store.get("test","home").unwrap().items[0]))}else{store.tool_edit("test","home",spatial_home::scene_tools::decode(&name,args).unwrap())})).await.unwrap();
 let saved=store.get("test","home").unwrap().items.remove(0);assert_eq!(saved.title,"Kitchen");assert_eq!(saved.description,"Keep me");assert_eq!(saved.size,[2.,2.,2.]);
}

#[tokio::test]
async fn stale_refs_wrong_types_frames_and_unknown_commands_never_dispatch(){
 let camera=json!([{"type":"peer","id":"phone"},{"type":"camera","reference":{"peer_id":"phone","product_id":"rgb","hash":"old"}}]);
 let mut wrong_frame=request("detach",json!([]));wrong_frame.frame="old-home".into();
 let mut wrong_scene=request("detach",json!([]));wrong_scene.scene="other".into();
 for r in [wrong_frame,wrong_scene,request("point",json!([])),request("invented",json!([])),request("delete_all_notes",json!([])),request("detach",json!([{"type":"text","value":"extra"}])),request("camera",camera),request("focus",json!([{"type":"peer","id":"offline"}])),request("focus",json!([{"type":"entity","kind":"note","id":"gone"}])),request("rename",json!([{"type":"entity","kind":"portal","id":"portal"},{"type":"text","value":"wrong"}]))] {
  let result=execute(&r,&inventory(),|_,_|{panic!("invalid request dispatched");#[allow(unreachable_code)]std::future::ready(Ok(json!({})))}).await;
  assert!(result.is_err());
 }
}

#[tokio::test]
async fn peer_focus_requires_live_generic_pose_then_moves_only_browser(){
 let r=request("focus",json!([{"type":"peer","id":"phone"}]));let mut calls=vec![];
 let result=execute(&r,&inventory(),|name,args|{
  calls.push(name.clone());std::future::ready(if name=="get_peer_pose"{Ok(json!({"location":{"position":[1,2,3]}}))}else{spatial_home::view_camera::action(args,&r.scene,&r.frame,r.viewer.as_ref())})
 }).await.unwrap();
 assert_eq!(calls,vec!["get_peer_pose","move_camera"]);assert_eq!(result["ui_actions"][0]["look_at"],json!([1.,2.,3.]));
 let mut count=0;assert!(execute(&r,&inventory(),|_,_|{count+=1;std::future::ready(Err(anyhow::anyhow!("Tracking lost")))}).await.is_err());assert_eq!(count,1);
}

#[tokio::test]
async fn a_write_failure_is_never_retried(){
 let mut a=vec![json!({"type":"text","value":"New note"})];a.extend(numbers());let r=request("note",json!(a));
 let mut count=0;assert!(execute(&r,&inventory(),|name,_|{assert_eq!(name,"create_note");count+=1;std::future::ready(Err(anyhow::anyhow!("write failed")))}).await.is_err());assert_eq!(count,1);
}

#[test]
fn strict_contract_rejects_untyped_fields(){
 assert!(serde_json::from_value::<Request>(json!({"scene":"a","frame":"b","command":"detach","args":[],"viewer":null,"tool":"delete_all_notes"})).is_err());
}

#[tokio::test]
async fn path_accepts_all_peer_entity_pairs_and_dispatches_once_with_exact_ids(){
 let peer=json!({"type":"peer","id":"phone"});let entity=json!({"type":"entity","kind":"note","id":"cheese"});
 for pair in [[peer.clone(),peer.clone()],[peer.clone(),entity.clone()],[entity.clone(),peer.clone()],[entity.clone(),entity.clone()]] {
  let r=request("path",json!(pair));let mut calls=0;
  let result=execute(&r,&inventory(),|name,args|{
   calls+=1;assert_eq!(name,"create_path");
   for i in 0..2 {assert_eq!(args["waypoints"][i]["id"],pair[i]["id"]);assert_eq!(args["waypoints"][i]["kind"],if pair[i]["type"]=="peer"{"peer"}else{"note"});}
   std::future::ready(Ok(json!({"summary":"Floor path saved","id":"path"})))
  }).await.unwrap();
  assert_eq!(calls,1);assert_eq!(result["changes"].as_array().unwrap().len(),1);assert_eq!(result["ui_actions"],json!([]));
 }
 for pair in [json!([peer,{"type":"entity","kind":"note","id":"missing"}]),json!([entity,{"type":"text","value":"Bracket 0125"}])] {
  assert!(execute(&request("path",pair),&inventory(),|_,_|{panic!("invalid path dispatched");#[allow(unreachable_code)]std::future::ready(Ok(json!({})))}).await.is_err());
 }
}

#[tokio::test]
async fn path_passes_many_ordered_waypoints_in_one_tool_invocation(){
 let args=json!([{"type":"entity","kind":"note","id":"cheese"},{"type":"peer","id":"phone"},{"type":"entity","kind":"volume","id":"room"},{"type":"entity","kind":"portal","id":"portal"}]);
 let mut calls=0;
 execute(&request("path",args),&inventory(),|name,args|{
  calls+=1;assert_eq!(name,"create_path");
  assert_eq!(args,json!({"waypoints":[{"kind":"note","id":"cheese"},{"kind":"peer","id":"phone"},{"kind":"volume","id":"room"},{"kind":"portal","id":"portal"}]}));
  std::future::ready(Ok(json!({"summary":"Path saved"})))
 }).await.unwrap();assert_eq!(calls,1);
 for count in [0,1,65] {assert!(execute(&request("path",json!(vec![json!({"type":"peer","id":"phone"});count])),&inventory(),|_,_|{panic!("invalid waypoint count dispatched");#[allow(unreachable_code)]std::future::ready(Ok(json!({})))}).await.is_err());}
}
