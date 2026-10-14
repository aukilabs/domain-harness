#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn geometry_inset_and_toward_viewer_feed_move_note()->anyhow::Result<()> {
 use serde_json::json;use spatial_home::{scene_geometry,semantic_volumes::Volume,chatbot::Viewer};
 let volume=Volume{id:"office-id".into(),title:"office".into(),description:"".into(),semantic_type:"region".into(),center:[0.,1.,0.],size:[10.,2.,10.],yaw:0.};
 let viewer=Viewer{frame:"frame".into(),mode:"3d".into(),screen_right:[1.,0.,0.],camera_position:Some([5.,2.,5.]),camera_target:None};
 for instruction in ["move the fancy car note one meter inside the farthest corner of the office","move the fancy car note one meter closer to me while staying on the ground"]{
  let note=json!({"id":"car-id","name":"fancy car","text":"fancy car","position":[0,0,0],"description":"keep this"});let mut calculated=None::<serde_json::Value>;let mut moved=false;
  let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:Some(viewer.clone()),scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:instruction.into()}]},|name,args|match name{
   "inspect_scenegraph"=>Ok(json!({"notes":[note],"semantic_volumes":{"items":[volume]}})),"get_note"=>Ok(note.clone()),"get_semantic_volume"=>Ok(json!(volume)),
   "get_volume_corners"=>scene_geometry::get_volume_corners(&volume,&viewer,"frame"),
   "point_inside_volume"=>{anyhow::ensure!(instruction.contains("inside")&&args["distance_m"]==1&&args["mode"]=="toward_center","Wrong inset: {args}");let v=scene_geometry::point_inside_volume(&volume,&viewer,args["corner"].as_str().unwrap(),1.,"toward_center","frame")?;calculated=Some(v["position"].clone());Ok(v)},
   "offset_point"=>{anyhow::ensure!(instruction.contains("closer")&&args["direction"]=="toward_viewer"&&args["distance_m"]==1&&args["ground"]==true,"Wrong offset: {args}");let v=scene_geometry::offset_point([0.;3],&viewer,"toward_viewer",1.,"viewer",true,"frame")?;calculated=Some(v["position"].clone());Ok(v)},
   "move_note"=>{let requested:[f64;3]=serde_json::from_value(args["position"].clone())?;let computed:[f64;3]=serde_json::from_value(calculated.clone().ok_or_else(||anyhow::anyhow!("Missing query"))?)?;let exact=scene_geometry::resolve_calculated_point(requested,&[computed])?;anyhow::ensure!(args["id"]=="car-id","Wrong note");moved=true;Ok(json!({"summary":"Moved note fancy car.","after":{"id":"car-id","position":exact,"description":"keep this"}}))},_=>anyhow::bail!("Query geometry then move_note")
  }).await?;assert!(moved,"{instruction}: {result}");
 }Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn geometry_query_then_existing_move_note()->anyhow::Result<()> {
 use serde_json::json;let mut queried=false;let mut moved=false;
 let note=json!({"id":"car-id","name":"fancy car","text":"fancy car","position":[0,0,0],"description":"keep this"});
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"move the fancy car note to the right corner of the office volume".into()}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"notes":[note],"semantic_volumes":{"items":[{"id":"office-id","title":"office"}]}})),
  "get_note"=>Ok(note.clone()),
  "get_volume_corners"=>{anyhow::ensure!(args["volume_id"]=="office-id","Wrong volume");queried=true;Ok(json!({"frame":"frame","labels":{"right":"x+:z-"},"corners":[{"id":"x+:z-","position":[7,0,-2]}]}))},
  "move_note"=>{anyhow::ensure!(queried&&args["id"]=="car-id"&&args["expected_position"]==json!([0,0,0])&&args["position"]==json!([7,0,-2]),"Incorrect move: {args}");moved=true;Ok(json!({"summary":"Moved note fancy car.","after":{"id":"car-id","description":"keep this","position":[7,0,-2]}}))},
  _=>anyhow::bail!("Query geometry then move_note; never create/delete a note"),
 }).await?;assert!(queried&&moved,"{result}");assert_eq!(result["changes"].as_array().unwrap().len(),1);Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_renames_wake_your_hands_despite_false_history()->anyhow::Result<()> {
 use serde_json::json;let mut renamed=false;
 let note=json!({"id":"note-test","name":"Wake your hands","text":"Wake your hands","position":[0,0,0],"authored":true});
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Change the name of Wake your hands to Wash your hands".into()},spatial_home::chatbot::Message{role:"assistant".into(),content:"Updated note name to Wash your hands.".into()},spatial_home::chatbot::Message{role:"user".into(),content:"Change the name of \"Wake your hands\" to Wash your hands".into()}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"notes":[note],"semantic_volumes":{"items":[]}})),
  "get_note"=>Ok(note.clone()),
  "rename_note"=>{anyhow::ensure!(args["id"]=="note-test"&&args["expected_name"]=="Wake your hands"&&args["name"]=="Wash your hands","Incorrect rename: {args}");renamed=true;Ok(json!({"summary":"Renamed note to Wash your hands.","after":{"name":"Wash your hands"}}))},
  _=>anyhow::bail!("Use rename_note"),
 }).await?;assert!(renamed,"{result}");assert_eq!(result["changes"].as_array().unwrap().len(),1);Ok(())
}
#[test]
fn note_update_success_claims_are_detected(){
 assert!(spatial_home::chatbot::claims_saved_edit("Updated note name to Wash your hands."));
 assert!(spatial_home::chatbot::claims_saved_edit("Updated note name Wake your hands to Wash your hands."));
 assert!(!spatial_home::chatbot::claims_saved_edit("Which note do you mean?"));
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_places_note_in_far_corner_without_questions()->anyhow::Result<()> {
 use serde_json::json;let mut created=false;
 let volume=json!({"id":"office-id","title":"office","center":[0,1,0],"size":[10,2,10],"yaw":0});
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Create a note saying test in teh far corner of the office".into()}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"notes":[],"semantic_volumes":{"items":[volume]}})),
  "get_semantic_volume"=>Ok(volume.clone()),
  "get_volume_corners"=>Ok(json!({"labels":{"far":"x-:z-"},"corners":[{"id":"x-:z-","position":[-5,0,-5]}]})),
  "create_note"=>{anyhow::ensure!(args["name"]=="test"&&args["position"]==json!([-5,0,-5]),"Wrong placement: {args}");created=true;Ok(json!({"summary":"Created note test.","after":{"position":[-5,0,-5]}}))},
  _=>anyhow::bail!("Query corners then create_note"),
 }).await?;assert!(created,"{result}");Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_names_peer_without_alias_jargon()->anyhow::Result<()> {
 use serde_json::json;let mut saved=false;
 let peer="12D3KooWHH5M8E69fZTMdutjqq4ektFHqnswvNiUrk6hTtaamwtL";
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:format!("I want to name the peer {peer} \"stevie\"")}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"peers":[{"peer_id":peer,"status":"connected"}],"peer_aliases":{},"notes":[],"semantic_volumes":{"items":[]}})),
  "set_peer_alias"=>{anyhow::ensure!(args["peer_id"]==peer&&args["alias"]=="stevie","Wrong peer or alias: {args}");saved=true;Ok(json!({"summary":"Named peer stevie."}))},
  _=>anyhow::bail!("Use set_peer_alias with the exact peer ID and requested alias"),
 }).await?;assert!(saved,"{result}");assert_eq!(result["changes"].as_array().unwrap().len(),1);Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_sets_synthetic_peer_alias()->anyhow::Result<()> {
 use serde_json::json;let mut saved=false;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Give Bracketbot 182 the alias Buddy".into()}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"peers":[{"peer_id":"synthetic-robot","name":"Bracketbot 182"}],"peer_aliases":{},"notes":[],"semantic_volumes":{"items":[]}})),
  "set_peer_alias"=>{anyhow::ensure!(args["peer_id"]=="synthetic-robot"&&args["alias"]=="Buddy","Wrong peer or alias");saved=true;Ok(json!({"summary":"Named peer Buddy."}))},
  _=>anyhow::bail!("Use set_peer_alias"),
 }).await?;assert!(saved,"{result}");Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_creates_named_volume_with_defaults()->anyhow::Result<()> {
 use serde_json::json;let mut created=false;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"assistant".into(),content:"I cannot create volumes because there is no creation tool.".into()},spatial_home::chatbot::Message{role:"user".into(),content:"Create a volume called office".into()}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"notes":[],"semantic_volumes":{"items":[]}})),
  "create_volume"=>{anyhow::ensure!(args["name"]=="office","Wrong volume name");anyhow::ensure!(args.get("center").is_none()&&args.get("size").is_none(),"Name-only creation must use server defaults");created=true;Ok(json!({"summary":"Created volume office.","after":{"title":"office","size":[1,1,1],"center":[0,0.5,0]}}))},
  _=>anyhow::bail!("Use create_volume"),
 }).await?;assert!(created,"{result}");Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_deletes_all_synthetic_notes()->anyhow::Result<()> {
 use serde_json::json;let mut deleted=false;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Delete all my notes".into()}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"notes":[{"id":"a","text":"Dan","authored":true},{"id":"b","text":"success","authored":true},{"id":"phone-name:test","text":"Phone","authored":false}],"semantic_volumes":{"items":[]}})),
  "delete_all_notes"=>{let ids=args["expected_ids"].as_array().unwrap();anyhow::ensure!(ids.len()==2&&ids.contains(&json!("a"))&&ids.contains(&json!("b")),"Must delete authored notes only");deleted=true;Ok(json!({"summary":"Deleted all 2 authored notes.","deleted_count":2}))},
  _=>anyhow::bail!("Use delete_all_notes"),
 }).await?;assert!(deleted,"{result}");Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_deletes_all_synthetic_volumes()->anyhow::Result<()> {
 use serde_json::json;
 let mut deleted=false;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Delete all my volumes".into()}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"semantic_volumes":{"items":[{"id":"a","title":"Office"},{"id":"b","title":"woohoo"}]}})),
  "delete_all_volumes"=>{let ids=args["expected_ids"].as_array().unwrap();anyhow::ensure!(ids.len()==2&&ids.contains(&json!("a"))&&ids.contains(&json!("b")),"Must delete both volumes");deleted=true;Ok(json!({"summary":"Deleted all 2 volumes in the selected scene.","deleted_count":2}))},
  _=>anyhow::bail!("Use delete_all_volumes for this request"),
 }).await?;
 assert!(deleted,"{result}");assert_eq!(result["changes"][0]["deleted_count"],2);Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_replaces_synthetic_description()->anyhow::Result<()> {
 use serde_json::json;
 let mut description="Old instruction".to_string();let mut writes=0;
 let id="00000000-0000-4000-8000-000000000001";
 let response=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Replace woohoo's entire description with: Say hello when you enter.".into()}]},|name,args|{
  let volume=json!({"id":id,"title":"woohoo","description":description});
  match name {
   "inspect_scenegraph"=>Ok(json!({"semantic_volumes":{"items":[volume]}})),
   "get_semantic_volume"=>{anyhow::ensure!(args["id"]==id,"Wrong ID");Ok(volume)},
   "update_volume_description"=>{anyhow::ensure!(args["id"]==id&&args["expected_description"]==description,"Wrong target or stale description");let before=description.clone();description=args["description"].as_str().unwrap().into();writes+=1;Ok(json!({"id":id,"title":"woohoo","before":before,"after":description}))},
   _=>anyhow::bail!("Unknown tool"),
  }
 }).await?;
 assert_eq!(description,"Say hello when you enter.","{response}");assert_eq!(writes,1);assert_eq!(response["changes"].as_array().unwrap().len(),1);Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_renames_synthetic_note()->anyhow::Result<()> {
 use serde_json::json;let mut renamed=false;
 let id="00000000-0000-4000-8000-000000000002";
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Rename the note Dan to Nils.".into()}]},|name,args|{
  let note=json!({"id":id,"name":"Dan","text":"Dan","position":[0,0,0],"description":"Keep this"});
  match name {
   "inspect_scenegraph"=>Ok(json!({"notes":[note],"semantic_volumes":{"items":[]}})),
   "get_note"=>Ok(note),
   "rename_note"=>{anyhow::ensure!(args["id"]==id&&args["expected_name"]=="Dan"&&args["name"]=="Nils","Incorrect rename: {args}");renamed=true;Ok(json!({"id":id,"title":"Nils","summary":"Renamed Dan to Nils."}))},
   _=>anyhow::bail!("Unexpected tool"),
  }
 }).await?;
 assert!(renamed,"{result}");Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_looks_up_and_lifts_volume()->anyhow::Result<()> {
 use serde_json::json;let mut moved=false;let mut inspected=false;
 let id="00000000-0000-4000-8000-000000000003";
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Find woohoo.".into()},spatial_home::chatbot::Message{role:"assistant".into(),content:"I cannot find woohoo. Provide its ID.".into()},spatial_home::chatbot::Message{role:"user".into(),content:"Lift woohoo by 50 cm.".into()}]},|name,args|{
  let volume=json!({"id":id,"title":"woohoo","description":"","center":[1,2,3],"size":[1,1,1],"yaw":0});
  match name {
   "inspect_scenegraph"=>{inspected=true;Ok(json!({"semantic_volumes":{"items":[volume]}}))},
   "get_semantic_volume"=>{inspected=true;Ok(volume)},
   "move_volume"=>{anyhow::ensure!(inspected&&args["id"]==id,"Missing lookup or wrong target");let center:[f64;3]=serde_json::from_value(args["center"].clone())?;let expected:[f64;3]=serde_json::from_value(args["expected_center"].clone())?;anyhow::ensure!(center==[1.,2.5,3.]&&expected==[1.,2.,3.],"Wrong coordinates: {args}");moved=true;Ok(json!({"id":id,"title":"woohoo","summary":"Raised woohoo by 50 cm."}))},
   _=>anyhow::bail!("Unexpected tool"),
  }
 }).await?;assert!(moved,"{result}");Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn consecutive_moves_use_current_coordinates()->anyhow::Result<()> {
 use serde_json::json;
 let mut center=[1.,2.,3.];let mut history=Vec::new();let id="00000000-0000-4000-8000-000000000004";
 for command in ["Lift woohoo by 50 cm.","Now lower it by 25 cm."] {
  let mut fresh=false;let mut moved=false;
  history.push(spatial_home::chatbot::Message{role:"user".into(),content:command.into()});
  let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:history.clone()},|name,args|{
   let v=json!({"id":id,"title":"woohoo","center":center,"size":[1,1,1],"description":"","yaw":0});
   match name {
    "inspect_scenegraph"=>Ok(json!({"semantic_volumes":{"items":[v]}})),
    "get_semantic_volume"=>{fresh=true;Ok(v)},
    "move_volume"=>{anyhow::ensure!(fresh,"Read current volume in this turn first");let expected:[f64;3]=serde_json::from_value(args["expected_center"].clone())?;anyhow::ensure!(expected==center,"Stale center; reread");let next:[f64;3]=serde_json::from_value(args["center"].clone())?;let before=center;center=next;moved=true;Ok(json!({"id":id,"title":"woohoo","summary":"Moved woohoo.","before":{"center":before},"after":{"center":center}}))},
    _=>anyhow::bail!("Unexpected tool"),
   }
  }).await?;
  assert!(moved,"{result}");history.push(spatial_home::chatbot::Message{role:"assistant".into(),content:format!("{}\n{}",result["reply"],result["changes"])});
 }
 assert_eq!(center,[1.,2.25,3.]);Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn exact_longest_axis_request_must_execute_resize()->anyhow::Result<()> {
 use serde_json::json;let id="de201b7d-baf5-4557-b364-03f9f5197dd2";let mut fresh=false;let mut resized=false;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"make woohoo 15 meters long in its longest axis".into()}]},|name,args|{
  let volume=json!({"id":id,"title":"woohoo","center":[1.1411,0.,-3.467],"size":[1.0113,2.,0.6],"description":"we are now in control.","yaw":0});
  match name {
   "inspect_scenegraph"=>Ok(json!({"semantic_volumes":{"items":[volume]}})),
   "get_semantic_volume"=>{fresh=true;Ok(volume)},
   "set_volume_length"=>{anyhow::ensure!(fresh&&args["id"]==id&&args["axis"]=="longest"&&args["length"]==15.,"Incorrect single-axis resize: {args}");let expected:[f64;3]=serde_json::from_value(args["expected_size"].clone())?;anyhow::ensure!(expected==[1.0113,2.,0.6],"Wrong expected size");resized=true;Ok(json!({"id":id,"title":"woohoo","summary":"Resized woohoo.","after":{"size":[1.0113,15.,0.6]}}))},
   _=>anyhow::bail!("Unexpected tool"),
  }
 }).await?;assert!(resized,"{result}");assert_eq!(result["changes"].as_array().unwrap().len(),1);Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn flatten_request_executes_height_tool()->anyhow::Result<()> {
 use serde_json::json;let id="00000000-0000-4000-8000-000000000006";let mut fresh=false;let mut wrote=false;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"flatten tmi so it is only 1 meter tall".into()}]},|name,args|{
  let volume=json!({"id":id,"title":"tmi","center":[0,0,0],"size":[3,4,5],"description":"","yaw":0});
  match name {
   "inspect_scenegraph"=>Ok(json!({"semantic_volumes":{"items":[volume]}})),
   "get_semantic_volume"=>{fresh=true;Ok(volume)},
   "set_volume_length"=>{anyhow::ensure!(fresh&&args["id"]==id&&args["axis"]=="y"&&args["length"]==1.,"Invalid flatten: {args}");wrote=true;Ok(json!({"id":id,"title":"tmi","summary":"Set tmi height to 1 meter.","after":{"size":[3,1,5]}}))},
   _=>anyhow::bail!("Use set_volume_length to change height only"),
  }
 }).await?;assert!(wrote,"{result}");Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn floor_placement_is_absolute_not_a_relative_lowering()->anyhow::Result<()> {
 use serde_json::json;let id="00000000-0000-4000-8000-000000000007";let mut fresh=false;let mut wrote=false;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Make TMI 1 meter tall.".into()},spatial_home::chatbot::Message{role:"assistant".into(),content:"Updated volume TMI.".into()},spatial_home::chatbot::Message{role:"user".into(),content:"place TMI 1 meter below the floor".into()}]},|name,args|{
  let volume=json!({"id":id,"title":"TMI","center":[1.109544,8.2680879,-1.1906646],"size":[2,1,4],"description":"","yaw":0});
  match name {
   "inspect_scenegraph"=>Ok(json!({"frame":"local:synthetic","notes":[{"id":"note-tmi","text":"TMI","position":[0,0,0]}],"semantic_volumes":{"items":[volume]}})),
   "get_semantic_volume"=>{fresh=true;Ok(volume)},
   "place_volume_relative_to_floor"=>{anyhow::ensure!(fresh&&args["id"]==id&&args["offset_m"]==-1.,"Incorrect floor placement: {args}");let expected:[f64;3]=serde_json::from_value(args["expected_center"].clone())?;anyhow::ensure!(expected==[1.109544,8.2680879,-1.1906646],"Wrong current center");wrote=true;Ok(json!({"id":id,"title":"TMI","summary":"Placed TMI bottom 1 meter below reference floor.","after":{"center":[1.109544,-0.5,-1.1906646]}}))},
   _=>anyhow::bail!("Use absolute floor placement, not relative move or note creation"),
  }
 }).await?;assert!(wrote,"{result}");Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn camera_raycast_then_create_note()->anyhow::Result<()> {
 use serde_json::json;
 for instruction in ["Place a note called camera test where Dan's phone is looking", "Place a note called camera test by raycasting pixel 320,400 in Dan's camera space"] {
  let mut queried=false;let mut created=false;
  let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:instruction.into()}]},|name,args|match name {
   "inspect_scenegraph"=>Ok(json!({"peers":[{"peer_id":"dan-peer","name":"Dan"}],"peer_aliases":{"dan-peer":"Dan"},"notes":[],"semantic_volumes":{"items":[]}})),
   "raycast_peer_camera"=>{anyhow::ensure!(args["peer_id"]=="dan-peer","Wrong peer");if instruction.contains("pixel"){anyhow::ensure!(args["pixel"]==json!([320,400]),"Wrong pixel");}else{anyhow::ensure!(args["pixel"].is_null(),"Expected optical axis");}queried=true;Ok(json!({"frame":"frame","position":[2,0,-4],"hit":{"kind":"floor","distance_m":4}}))},
   "create_note"=>{anyhow::ensure!(queried&&args["position"]==json!([2,0,-4]),"Must use raycast hit");created=true;Ok(json!({"summary":"Created note camera test.","after":{"position":[2,0,-4]}}))},
   _=>anyhow::bail!("Raycast then create_note"),
  }).await?;assert!(queried&&created,"{result}");
 }Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_moves_view_camera_without_scene_edits()->anyhow::Result<()> {
 use serde_json::json;
 let viewer:spatial_home::chatbot::Viewer=serde_json::from_value(json!({"frame":"synthetic-frame","mode":"3d","screen_right":[1,0,0],"camera_position":[3,4,5],"camera_target":[0,0,0]}))?;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{
  viewer:Some(viewer.clone()),scene:"local:synthetic".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Move the map camera to [2,3,4] looking at [0,1,0].".into()}]
 },|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"notes":[],"semantic_volumes":{"items":[]}})),
  "move_camera"=>{let position:[f64;3]=serde_json::from_value(args["position"].clone())?;let target:[f64;3]=serde_json::from_value(args["look_at"].clone())?;anyhow::ensure!(position==[2.,3.,4.]&&target==[0.,1.,0.],"Wrong view: {args}");spatial_home::view_camera::action(args,"local:synthetic","synthetic-frame",Some(&viewer))},
  _=>anyhow::bail!("Only a browser-local camera move is authorized"),
 }).await?;
 assert_eq!(result["changes"],json!([]));
 assert_eq!(result["ui_actions"][0]["type"],"move_camera","{result}");
 Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_attaches_sensor_view_camera()->anyhow::Result<()> {
 use serde_json::json;
 let viewer=serde_json::from_value(json!({"frame":"home-frame","mode":"3d","screen_right":[1,0,0],"camera_position":[3,4,5],"camera_target":[0,0,0]}))?;
 let mut inspected=false;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:Some(viewer),scene:"local:home".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Attach the map camera to Dan's Phone so I see the world from its POV.".into()}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"notes":[],"semantic_volumes":{"items":[]},"peers":[{"peer_id":"phone-peer"}],"peer_aliases":{"phone-peer":"Dan's Phone"}})),
  "inspect_catalog"=>Ok(json!({"section":"peers","page":{"items":[{"peer_id":"phone-peer","alias":"Dan's Phone","catalog_status":"ready"}],"next_offset":null}})),
  "inspect_sensor_frames"=>{anyhow::ensure!(args["peer_id"]=="phone-peer","Wrong peer");inspected=true;Ok(json!({"frames":[{"peer_id":"phone-peer","frame_id":"phone-camera-optical","pose_product":{"product_id":"camera-pose-history"}},{"peer_id":"phone-peer","frame_id":"phone-body","pose_product":{"product_id":"body-pose-history"}}]}))},
  "attach_camera"=>{anyhow::ensure!(inspected&&args["peer_id"]=="phone-peer"&&args["frame_id"]=="phone-camera-optical","Wrong attachment: {args}");Ok(json!({"type":"attach_camera","scene":"local:home","frame":"home-frame","peer_id":"phone-peer","frame_id":"phone-camera-optical"}))},
  _=>anyhow::bail!("Use sensor pose attachment, not video, hardware commands or scene edits"),
 }).await?;
 assert_eq!(result["ui_actions"][0]["type"],"attach_camera","{result}");assert_eq!(result["changes"],json!([]));Ok(())
}

#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_where_is_reveals_note_without_moving_it()->anyhow::Result<()> {
 use serde_json::json;
 let viewer:spatial_home::chatbot::Viewer=serde_json::from_value(json!({"frame":"home-frame","mode":"3d","screen_right":[1,0,0],"camera_position":[3,4,5],"camera_target":[0,0,0]}))?;
 let mut read=false;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:Some(viewer.clone()),scene:"local:home".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Where is this is dan?".into()}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"notes":[{"id":"dan-note","name":"this is dan","text":"this is dan","authored":true}],"semantic_volumes":{"items":[]}})),
  "get_note"=>{anyhow::ensure!(args["id"]=="dan-note","Wrong note: {args}");read=true;Ok(json!({"id":"dan-note","text":"this is dan","position":[8,1,-3],"authored":true}))},
  "move_camera"=>{anyhow::ensure!(read,"Must read actual target position");let target:[f64;3]=serde_json::from_value(args["look_at"].clone())?;let p:[f64;3]=serde_json::from_value(args["position"].clone())?;
   anyhow::ensure!(target==[8.,1.,-3.],"Must aim at the note: {args}");let distance=p.iter().zip(target).map(|(a,b)|(a-b).powi(2)).sum::<f64>().sqrt();anyhow::ensure!((1.0..=12.).contains(&distance)&&p[1]>target[1],"Expected a useful elevated view: {args}");
   spatial_home::view_camera::action(args,"local:home","home-frame",Some(&viewer))},
  _=>anyhow::bail!("Only read target and move browser viewpoint, not scene edits or video"),
 }).await?;
 assert_eq!(result["ui_actions"][0]["type"],"move_camera","{result}");assert_eq!(result["changes"],json!([]));Ok(())
}

#[tokio::test]
#[ignore = "requires local Ollama with configured vision Qwen model"]
async fn local_model_describes_selected_camera_image()->anyhow::Result<()> {
 use serde_json::json;
 let mut captured=false;
 let image=spatial_home::camera_vision::encode(&auki_components::VideoFrame{width:128,height:128,encoding:"rgb8".into(),bytes:vec![255,0,0].repeat(128*128).into()})?;
 let result=spatial_home::chatbot::chat_with_camera(spatial_home::chatbot::Request{viewer:None,scene:"local:home".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Describe what Steve 182's left wrist camera is looking at.".into()}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"notes":[],"semantic_volumes":{"items":[]},"peers":[],"peer_aliases":{"robot-peer":"Steve 182"}})),
  "inspect_catalog"=>if args["peer_id"].is_null(){Ok(json!({"section":"peers","page":{"items":[{"peer_id":"robot-peer","alias":"Steve 182","catalog_status":"ready"}],"next_offset":null}}))}else{anyhow::ensure!(args["peer_id"]=="robot-peer","Wrong peer");Ok(json!({"section":"products","available":true,"page":{"items":[{"manifest":{"product_id":"camera.wrist.left","payload":"VideoFrame"}},{"manifest":{"product_id":"camera.wrist.right","payload":"VideoFrame"}}],"next_offset":null}}))},
  _=>anyhow::bail!("Descriptions must not open PiP, raycast, edit scenes or move cameras"),
 },|args|{captured=true;let image=image.clone();async move {anyhow::ensure!(args["peer_id"]=="robot-peer"&&args["product_id"]=="camera.wrist.left","Wrong camera: {args}");Ok(json!({"image":image,"peer_id":"robot-peer","product":{"product_id":"camera.wrist.left"},"capture_timestamp_ns":100}))}}).await?;
 assert!(captured,"{result}");assert!(result["reply"].as_str().unwrap().to_lowercase().contains("red"),"Must actually see the red image: {result}");
 assert_eq!(result["changes"],json!([]));assert!(result.get("ui_actions").is_none());assert!(result["camera_frame"].get("image").is_none());Ok(())
}

#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn local_model_where_is_phone_uses_live_position_not_attachment()->anyhow::Result<()> {
 use serde_json::json;
 let viewer:spatial_home::chatbot::Viewer=serde_json::from_value(json!({"frame":"home-frame","mode":"3d","screen_right":[1,0,0],"camera_position":[3,4,5],"camera_target":[0,0,0]}))?;
 let mut read_pose=false;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:Some(viewer.clone()),scene:"local:home".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"Where is Dan's Phone?".into()}]},|name,args|match name{
  "inspect_scenegraph"=>Ok(json!({"notes":[],"semantic_volumes":{"items":[]},"peers":[{"peer_id":"phone","alias":"Dan's Phone","status":"connected"},{"peer_id":"offline-peer","alias":"Offline robot","status":"disconnected"}],"peer_aliases":{"phone":"Dan's Phone"}})),
  "get_peer_pose"=>{anyhow::ensure!(args==json!({"peer_id":"phone"}),"Read only the requested peer: {args}");read_pose=true;Ok(json!({"peer_id":"phone","location":{"position":[8,1,-3],"rotation_xyzw":[0,0,0,1],"frame":"home-frame","received_age_ms":50,"capture_timestamp_ns":"1700000000000000003","clock":{"peer_id":"phone","id":"capture","hash":"fixture-clock"},"freshness":{"status":"live","basis":"arrival","maximum_received_age_ms":2000}}}))},
  "move_camera"=>{anyhow::ensure!(read_pose,"Must read a targeted current pose before choosing the viewpoint");let target:[f64;3]=serde_json::from_value(args["look_at"].clone())?;anyhow::ensure!(target==[8.,1.,-3.],"Must use live position: {args}");spatial_home::view_camera::action(args,"local:home","home-frame",Some(&viewer))},
  _=>anyhow::bail!("Use current peer location, not attachment or scene edits"),
 }).await?;
 assert!(read_pose,"{result}");assert_eq!(result["ui_actions"][0]["type"],"move_camera","{result}");assert_eq!(result["changes"],json!([]));Ok(())
}
#[tokio::test]
#[ignore = "requires local Ollama with configured Qwen model"]
async fn live_peer_distance_is_answered_without_scene_edits()->anyhow::Result<()> {
 use serde_json::json;
 let mut read_pose=false;
 let result=spatial_home::chatbot::chat(spatial_home::chatbot::Request{viewer:None,scene:"local:fixture".into(),messages:vec![spatial_home::chatbot::Message{role:"user".into(),content:"How far is Broom boy from cheese?".into()}]},|name,args|match name {
  "inspect_scenegraph"=>Ok(json!({"frame":"fixture","notes":[{"id":"cheese","text":"cheese","position":[6,0,8]}],"peers":[{"peer_id":"synthetic-peer","alias":"Broom boy","status":"connected"},{"peer_id":"unrelated-peer","alias":"Another robot","status":"disconnected"}],"semantic_volumes":{"items":[]}})),
  "get_peer_pose"=>{anyhow::ensure!(args==json!({"peer_id":"synthetic-peer"}),"Read only Broom boy: {args}");read_pose=true;Ok(json!({"peer_id":"synthetic-peer","location":{"position":[0,0,0],"rotation_xyzw":[0,0,0,1],"frame":"fixture","received_age_ms":10,"capture_timestamp_ns":"1700000000000000003","clock":{"peer_id":"synthetic-peer","id":"capture","hash":"fixture-clock"},"freshness":{"status":"live","basis":"arrival","maximum_received_age_ms":2000}}}))},
  "get_note"=>Ok(json!({"id":"cheese","name":"cheese","position":[6,0,8]})),
  _=>anyhow::bail!("Read-only question must not move a view, open video or edit scene"),
 }).await?;
 assert!(read_pose,"{result}");assert!(result["changes"].as_array().unwrap().is_empty());assert!(result.get("ui_actions").is_none());assert!(result["reply"].as_str().unwrap().contains("10"),"{result}");Ok(())
}
