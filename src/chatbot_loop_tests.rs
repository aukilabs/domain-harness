//! Deterministic controller tests: no Ollama, live scenes or external peers.
use super::*;
use serde_json::Value;
use std::{collections::VecDeque, future::ready};

fn request(text:&str)->Request {
 Request {scene:"test-scene".into(),viewer:None,messages:vec![Message {role:"user".into(),content:text.into()}]}
}
fn call(name:&str,args:Value)->Value {json!({"function":{"name":name,"arguments":args}})}
fn calls(calls:Vec<Value>)->Result<Value> {Ok(json!({"message":{"role":"assistant","content":"","tool_calls":calls}}))}
fn plain_answer(text:&str)->Result<Value> {Ok(json!({"message":{"role":"assistant","content":text}}))}
fn answer(text:&str)->Result<Value> {
 if text.starts_with('{') {plain_answer(text)}else{plain_answer(&json!({"answer":text,"explanation":""}).to_string())}
}
fn create()->Value {call("create_note",json!({"name":"cheese","description":"","position":[1,0,2]}))}
fn view()->Value {call("move_camera",json!({"position":[3,2,4],"look_at":[1,0,2]}))}
fn snapshot()->Value {json!({"notes":[],"semantic_volumes":{"items":[]},"peers":[{"peer_id":"unrelated-offline","status":"unavailable"},{"peer_id":"requested","alias":"Broom boy","status":"connected"}]})}

async fn script(text:&str,mut replies:VecDeque<Result<Value>>,mut handler:impl FnMut(&str,Value)->Result<Value>)->(Value,Vec<(String,Value)>,Vec<Vec<Value>>) {
 let mut executed=Vec::new();let mut prompts=Vec::new();
 let result=run_chat(request(text),"fixture".into(),|name,args| {
  executed.push((name.clone(),args.clone()));
  ready(if name=="inspect_scenegraph" {Ok(snapshot())}else{handler(&name,args)})
 },|_|ready(Err(anyhow::anyhow!("No camera expected"))),|messages,_tools| {
  prompts.push(messages);
  ready(replies.pop_front().expect("unexpected model round"))
 }).await.unwrap();
 (result,executed,prompts)
}
fn receipt(name:&str,args:Value)->Result<Value> {
 if is_ui_action(name) {Ok(json!({"type":name,"scene":"test-scene","frame":"test-frame","args":args}))}
 else {Ok(json!({"id":"note-id","summary":format!("Saved {name}"),"after":args}))}
}

#[tokio::test]
async fn final_answers_separate_the_conclusion_from_supporting_calculations() {
 let conclusion="F26ZSPXNEO7 and NV16FWJ9G2X are closest, about 1.19 m apart.";
 let explanation="Compared all three Euclidean distances: 2.08 m, 1.19 m and 1.45 m.";
 let (result,executed,prompts)=script("Which portals are closest?",VecDeque::from([
  answer(&json!({"answer":conclusion,"explanation":explanation}).to_string())
 ]),|_,_|panic!("No effects expected")).await;
 assert_eq!(result["reply"],conclusion);assert_eq!(result["explanation"],explanation);
 assert_eq!(executed.len(),1);
 assert!(prompts[0].iter().any(|m|m["content"]==FINAL_REPLY_GUIDANCE));
 assert!(!prompts[0].iter().any(|m|m["content"]==ANSWER_FORMAT));
}

#[tokio::test]
async fn long_plain_answers_get_one_tool_free_formatting_attempt() {
 let long="Here are the distances.\nA to B: 1 m. A to C: 3 m. B to C: 2 m.\nA and B are closest.";
 let mut responses=VecDeque::from([plain_answer(long),answer(r#"{"answer":"A and B are closest, 1 m apart.","explanation":"All pairs: AB 1 m, AC 3 m, BC 2 m."}"#)]);
 let mut prompts=Vec::new();
 let result=run_chat(request("Which pair is closest?"),"fixture".into(),|_,_|ready(Ok(snapshot())),|_|ready(Err(anyhow::anyhow!("No camera expected"))),|messages,tools|{
  prompts.push((messages,tools));ready(responses.pop_front().unwrap())
 }).await.unwrap();
 assert_eq!(result["reply"],"A and B are closest, 1 m apart.");
 assert_eq!(prompts.len(),2);assert_eq!(prompts[1].1,json!([]));
 assert!(prompts[0].0.iter().any(|m|m["content"]==FINAL_REPLY_GUIDANCE));
 assert!(!prompts[0].0.iter().any(|m|m["content"]==ANSWER_FORMAT));
 assert!(prompts[1].0.iter().any(|m|m["content"].as_str().is_some_and(|s|s.starts_with(ANSWER_FORMAT))));
 assert_eq!(prompts[1].0.last().unwrap()["content"],long);
 assert!(prompts[1].0.iter().any(|m|m["content"].as_str().is_some_and(|s|s.contains("CURRENT SAVED STATE"))));
}

#[tokio::test]
async fn plain_short_answers_still_get_a_chance_to_supply_an_explanation() {
 let (result,executed,prompts)=script("Which pair is closest?",VecDeque::from([
  plain_answer("A and B, 1 m apart."),answer(r#"{"answer":"A and B, 1 m apart.","explanation":"The other two pairs are 2 m and 3 m apart."}"#)
 ]),|_,_|panic!("Formatting must not dispatch tools")).await;
 assert_eq!(executed.len(),1);assert_eq!(prompts.len(),2);
 assert_eq!(result["reply"],"A and B, 1 m apart.");
 assert!(result["explanation"].as_str().unwrap().contains("2 m and 3 m"));
}

#[tokio::test]
async fn malformed_formatting_does_not_hide_the_original_or_execute_tools() {
 let original="First paragraph.\nSecond paragraph.\nThe conclusion is uncertain.";
 for formatted in [answer("{\"answer\":"),calls(vec![create()]),Err(anyhow::anyhow!("Formatting timed out"))] {
  let (result,executed,prompts)=script("Explain the scene",VecDeque::from([plain_answer(original),formatted]),|_,_|panic!("Must not execute formatter tools")).await;
  assert_eq!(executed.len(),1);assert_eq!(prompts.len(),2);
  assert_eq!(result["explanation"],original);
  assert!(result["reply"].as_str().unwrap().contains("could not be shortened"));
 }
}

#[tokio::test]
async fn explanations_cannot_smuggle_unconfirmed_edit_claims() {
 let unsupported=r#"{"answer":"Done.","explanation":"Created note cheese and a volume."}"#;
 let (result,_,_)=script("Create a note",VecDeque::from([answer(unsupported),answer(unsupported),answer(unsupported)]),|_,_|panic!("No tool call provided")).await;
 assert_eq!(result["status"],"incomplete");assert!(result.get("explanation").is_none());
 assert!(result["reply"].as_str().unwrap().contains("without a saved receipt"));
 let (result,_,_)=script("Create a note",VecDeque::from([calls(vec![create()]),answer(unsupported)]),receipt).await;
 assert_eq!(result["reply"],"Saved create_note");assert!(result.get("explanation").is_none());
}

#[tokio::test]
async fn compact_vision_keeps_both_parts_out_of_the_action_planner() {
 let mut responses=VecDeque::from([
  calls(vec![call("capture_camera_frame",json!({"peer_id":"requested","product_id":"camera"}))]),
  answer(r#"{"answer":"A yellow chair.","explanation":"IMAGE-ONLY-EVIDENCE"}"#),answer("Finished.")
 ]);
 let mut prompts=Vec::new();
 let result=run_chat(request("Describe the camera"),"fixture".into(),|_,_|ready(Ok(snapshot())),|_|ready(Ok(json!({"image":"fixture","peer_id":"requested"}))),|messages,_|{
  prompts.push(messages);ready(responses.pop_front().unwrap())
 }).await.unwrap();
 assert_eq!(result["reply"],"A yellow chair.");assert_eq!(result["explanation"],"IMAGE-ONLY-EVIDENCE");
 assert_eq!(result["camera_description"],"A yellow chair.");
 let planner=serde_json::to_string(&prompts[2]).unwrap();
 assert!(!planner.contains("yellow chair"));assert!(!planner.contains("IMAGE-ONLY-EVIDENCE"));
}

#[tokio::test]
async fn portal_inventory_keeps_anchors_frame_and_room_geometry() {
 let scene=json!({"frame":"home-frame","anchors":[{"portal_id":"portal-A","position":[1,0,2]}],"semantic_volumes":{"items":[{"id":"kitchen-id","title":"Kitchen","center":[1,1,2],"size":[4,2,6],"yaw":0.5}]}});
 let mut prompts=Vec::new();
 run_chat(request("Is there a portal in the Kitchen?"),"fixture".into(),|_,_|ready(Ok(scene.clone())),|_|ready(Err(anyhow::anyhow!("No camera"))),|messages,_|{
  prompts=messages;ready(answer("Yes."))
 }).await.unwrap();
 let state=prompts.iter().filter_map(|m|m["content"].as_str()).find(|s|s.starts_with("CURRENT SAVED STATE")).unwrap();
 assert!(state.contains("portal-A"));assert!(state.contains("home-frame"));assert!(state.contains("\"size\":[4,2,6]"));assert!(state.contains("\"yaw\":0.5"));
 assert!(prompts.iter().any(|m|m["content"].as_str().is_some_and(|s|s.contains("Missing fields are not evidence")&&s.contains("without asking permission")&&s.contains("already called inspect_scenegraph"))));
}

#[tokio::test]
async fn robot_pointing_reports_the_robot_receipt_not_model_prose() {
 let point=call("point_robot_at",json!({"robot":"Broom boy","target":{"kind":"position","position":"[1,0,2]"}}));
 let (result,executed,_)=script("Have Broom boy point at [1,0,2]",VecDeque::from([
  calls(vec![point.clone()]),calls(vec![point]),answer("Broom boy is now pointing and waving.")
 ]),|name,args|{
  assert_eq!(args["target"]["position"],json!([1.,0.,2.]));
  Ok(json!({"summary":format!("{name}: Broom boy accepted the point. The arm did not move."),"robot_peer_id":"requested"}))
 }).await;
 // Executed once despite the repeated call; the reply is the robot's receipt.
 assert_eq!(executed.iter().filter(|(name,_)|name=="point_robot_at").count(),1);
 assert_eq!(result["robot_actions"].as_array().unwrap().len(),1);
 assert!(result["reply"].as_str().unwrap().contains("The arm did not move"),"{result}");
 assert!(!result["reply"].as_str().unwrap().contains("waving"));
 assert_eq!(result["status"],"finished");
}

#[tokio::test]
async fn create_then_show_completes_both_and_feeds_the_saved_receipt_back() {
 let (result,executed,prompts)=script("Create a cheese note at [1,0,2], then show it",VecDeque::from([
  calls(vec![create()]),calls(vec![view()]),answer("Saved the note and requested its view.")
 ]),receipt).await;
 assert_eq!(executed.iter().map(|(name,_)|name.as_str()).collect::<Vec<_>>(),["inspect_scenegraph","create_note","move_camera"]);
 assert_eq!(result["changes"].as_array().unwrap().len(),1);
 assert_eq!(result["ui_actions"][0]["type"],"move_camera");
 assert_eq!(result["status"],"finished");
 assert!(prompts[1].iter().any(|m|m["role"]=="tool"&&m["tool_name"]=="create_note"&&m["content"].as_str().unwrap().contains("note-id")));
}

#[tokio::test]
async fn batched_writes_and_browser_actions_preserve_order() {
 let (result,executed,_)=script("Create two notes, detach, then show them",VecDeque::from([
  calls(vec![create(),call("create_note",json!({"name":"bread","description":"","position":[2,0,2]})),call("detach_camera",json!({})),view()]),answer("Done.")
 ]),receipt).await;
 assert_eq!(executed.len(),5);
 assert_eq!(result["changes"].as_array().unwrap().len(),2);
 assert_eq!(result["ui_actions"][0]["type"],"detach_camera");
 assert_eq!(result["ui_actions"][1]["type"],"move_camera");
}

#[tokio::test]
async fn duplicate_actions_are_not_replayed_and_reads_can_repeat() {
 let (result,executed,_)=script("Create a note and show it",VecDeque::from([
  calls(vec![create(),view()]),calls(vec![create(),view(),call("get_note",json!({"id":"note-id"}))]),
  calls(vec![call("get_note",json!({"id":"note-id"}))]),answer("Done.")
 ]),receipt).await;
 for name in ["create_note","move_camera"] {assert_eq!(executed.iter().filter(|(n,_)|n==name).count(),1);}
 assert_eq!(executed.iter().filter(|(n,_)|n=="get_note").count(),2);
 assert_eq!(result["changes"].as_array().unwrap().len(),1);
 assert_eq!(result["ui_actions"].as_array().unwrap().len(),1);
}

#[tokio::test]
async fn failed_write_is_not_automatically_retried_and_model_cannot_claim_success() {
 let (result,executed,_)=script("Create a note",VecDeque::from([
  calls(vec![create()]),calls(vec![create()]),answer("Everything was saved.")
 ]),|_,_|Err(anyhow::anyhow!("Write confirmation unavailable"))).await;
 assert_eq!(executed.iter().filter(|(n,_)|n=="create_note").count(),1);
 assert_eq!(result["status"],"incomplete");
 assert!(result["changes"].as_array().unwrap().is_empty());
 assert!(result["reply"].as_str().unwrap().contains("Write confirmation unavailable"));
 assert!(!result["reply"].as_str().unwrap().contains("Everything was saved"));
}

#[tokio::test]
async fn later_failures_preserve_saved_receipts_and_queued_actions() {
 for failure in [Err(anyhow::anyhow!("Ollama stopped")),calls(vec![call("move_camera",json!([]))]),calls(vec![view();5])] {
  let (result,_,_)=script("Create a note and show it",VecDeque::from([
   calls(vec![create(),call("detach_camera",json!({}))]),failure,answer("Done.")
  ]),receipt).await;
  assert_eq!(result["changes"].as_array().unwrap().len(),1);
  assert_eq!(result["ui_actions"][0]["type"],"detach_camera");
  assert_eq!(result["status"],"partial");
  assert!(!result["errors"].as_array().unwrap().is_empty());
  assert!(!result["reply"].as_str().unwrap().contains("No scene change was saved"));
 }
}

#[tokio::test]
async fn planning_and_effect_limits_return_receipts_instead_of_losing_them() {
 let mut replies=VecDeque::from([calls(vec![create()])]);
 for _ in 0..7 {replies.push_back(calls(vec![call("inspect_scenegraph",json!({}))]));}
 let (result,_,_)=script("Create a note",replies,receipt).await;
 assert_eq!(result["changes"].as_array().unwrap().len(),1);
 assert_eq!(result["status"],"partial");
 assert!(result["reply"].as_str().unwrap().contains("eight-round"));

 let mut replies=VecDeque::new();
 for batch in 0..3 {replies.push_back(calls((0..4).map(|i|call("create_note",json!({"name":format!("note-{}",batch*4+i),"description":"","position":[0,0,0]}))).collect()));}
 let (result,executed,_)=script("Create notes",replies,receipt).await;
 assert_eq!(result["changes"].as_array().unwrap().len(),8);
 assert_eq!(executed.iter().filter(|(n,_)|n=="create_note").count(),8);
 assert_eq!(result["status"],"partial");
}

#[tokio::test]
async fn metadata_inventory_never_requests_unrelated_live_poses() {
 let (result,executed,_)=script("What is in the Catalog?",VecDeque::from([answer("Two peers are advertised.")]),|_,_|panic!("should not resolve any peer")).await;
 assert_eq!(executed.len(),1);assert_eq!(executed[0].0,"inspect_scenegraph");
 assert_eq!(result["status"],"finished");
 let (_,executed,prompts)=script("Where is Broom boy?",VecDeque::from([
  calls(vec![call("get_peer_pose",json!({"peer_id":"requested"}))]),calls(vec![view()]),answer("Requested its viewpoint.")
 ]),|name,args|if name=="get_peer_pose" {assert_eq!(args["peer_id"],"requested");Ok(json!({"peer_id":"requested","location":{"position":[1,0,2],"frame":"test-frame","received_age_ms":5}}))}else{receipt(name,args)}).await;
 assert_eq!(executed.iter().filter(|(n,_)|n=="get_peer_pose").count(),1);
 assert!(prompts[0].iter().any(|m|m["content"].as_str().is_some_and(|s|s.contains("NOT current positions"))));
}

#[tokio::test]
async fn disabled_and_unknown_tools_are_not_dispatched() {
 let (result,executed,_)=script("Hello",VecDeque::from([
  calls(vec![create(),call("drive_robot",json!({}))]),answer("Could not do that.")
 ]),|_,_|panic!("disabled tool must not execute")).await;
 assert_eq!(executed.len(),1);assert_eq!(result["errors"].as_array().unwrap().len(),2);
}

#[tokio::test]
async fn one_discovery_call_allows_related_steps_without_erasing_geometry() {
 let (result,executed,prompts)=script("Make something",VecDeque::from([
  calls(vec![call("enable_scene_tools",json!({"category":"geometry"}))]),
  calls(vec![call("enable_scene_tools",json!({"category":"notes"}))]),
  calls(vec![create()]),calls(vec![call("rename_note",json!({"id":"note-id","expected_name":"cheese","name":"bread"}))]),answer("Saved both edits.")
 ]),receipt).await;
 assert_eq!(result["changes"].as_array().unwrap().len(),2);
 assert!(!executed.iter().any(|(n,_)|n=="enable_scene_tools"));
 assert!(prompts[2].iter().any(|m|m["tool_name"]=="enable_scene_tools"));
}

#[tokio::test]
async fn camera_vision_is_tool_free_and_never_enters_the_action_planner() {
 let mut responses=VecDeque::from([
  calls(vec![call("capture_camera_frame",json!({"peer_id":"requested","product_id":"camera"})),create()]),
  answer("IMAGE-DERIVED-UNTRUSTED-TEXT"),answer("Saved the note.")
 ]);
 let mut prompts=Vec::new();let mut executed=Vec::new();
 let result=run_chat(request("Describe the camera and create a note at [1,0,2]"),"fixture".into(),|name,args| {
  executed.push(name.clone());ready(if name=="inspect_scenegraph" {Ok(snapshot())}else{receipt(&name,args)})
 },|_|ready(Ok(json!({"image":"base64-test-image","peer_id":"requested","product_id":"camera"}))),|messages,tools| {
  prompts.push((messages,tools));ready(responses.pop_front().unwrap())
 }).await.unwrap();
 assert_eq!(prompts[1].1,json!([]));
 assert_eq!(prompts[1].0[1]["images"][0],"base64-test-image");
 let planner=serde_json::to_string(&prompts[2]).unwrap();
 assert!(!planner.contains("IMAGE-DERIVED-UNTRUSTED-TEXT"));assert!(!planner.contains("base64-test-image"));
 assert!(result["reply"].as_str().unwrap().contains("IMAGE-DERIVED-UNTRUSTED-TEXT"));
 assert_eq!(result["changes"].as_array().unwrap().len(),1);
 assert!(result["camera_frame"].get("image").is_none());
 assert_eq!(executed,["inspect_scenegraph","create_note"]);
}

#[tokio::test]
async fn one_receipt_does_not_authorize_unexecuted_success_claims() {
 let (result,_,_)=script("Create a cheese note and a bread volume",VecDeque::from([
  calls(vec![create()]),answer("Created both the cheese note and bread volume.")
 ]),receipt).await;
 assert_eq!(result["changes"].as_array().unwrap().len(),1);
 assert!(result["reply"].as_str().unwrap().contains("Saved create_note"));
 assert!(!result["reply"].as_str().unwrap().contains("bread volume"));
}

#[tokio::test]
async fn recovered_errors_do_not_hide_a_read_answer_or_an_edit_receipt() {
 let mut attempts=0;
 let (result,_,_)=script("Where is Broom boy?",VecDeque::from([
  calls(vec![call("get_peer_pose",json!({"peer_id":"requested"}))]),
  calls(vec![call("get_peer_pose",json!({"peer_id":"requested"}))]),answer("Broom boy is at [1,0,2].")
 ]),|_,_|{attempts+=1;if attempts==1 {Err(anyhow::anyhow!("Waiting for fresh sensor poses"))}else{Ok(json!({"location":{"position":[1,0,2]}}))}}).await;
 assert_eq!(result["reply"],"Broom boy is at [1,0,2].");
 assert_eq!(result["status"],"finished");
 assert!(result.get("errors").is_none());assert_eq!(result["recovered_errors"].as_array().unwrap().len(),1);

 let (result,_,_)=script("Create a note",VecDeque::from([
  calls(vec![call("create_note",json!([]))]),calls(vec![create()]),answer("Done.")
 ]),receipt).await;
 assert_eq!(result["status"],"finished");assert_eq!(result["changes"].as_array().unwrap().len(),1);
 let (result,_,_)=script("Hello",VecDeque::from([
  calls(vec![call("enable_scene_tools",json!({"category":"note"}))]),
  calls(vec![call("enable_scene_tools",json!({"category":"notes"}))]),answer("The notes tools are available.")
 ]),|_,_|panic!("discovery is internal, not an edit")).await;
 assert_eq!(result["status"],"finished");assert!(result.get("errors").is_none());
}

#[tokio::test]
async fn success_for_one_target_does_not_erase_another_targets_error() {
 let (result,_,_)=script("Where are the peers?",VecDeque::from([
  calls(vec![call("get_peer_pose",json!({"peer_id":"unrelated-offline"})),call("get_peer_pose",json!({"peer_id":"requested"}))]),answer("Both are available.")
 ]),|_,args|if args["peer_id"]=="unrelated-offline" {Err(anyhow::anyhow!("Peer unavailable"))}else{Ok(json!({"location":{"position":[1,0,2]}}))}).await;
 assert_eq!(result["errors"].as_array().unwrap().len(),1);
 assert_eq!(result["status"],"incomplete");
 assert!(result["reply"].as_str().unwrap().contains("Peer unavailable"));
}

#[tokio::test]
async fn deferred_browser_move_cannot_silently_change_view_relative_geometry() {
 let (result,executed,_)=script("Move the camera, then create a note to my right",VecDeque::from([
  calls(vec![view(),call("create_note_relative_to_view",json!({"reference_kind":"note","reference_id":"existing","name":"cheese","description":"","direction":"right","distance_m":1}))]),answer("Done.")
 ]),receipt).await;
 assert_eq!(result["ui_actions"].as_array().unwrap().len(),1);
 assert!(result["changes"].as_array().unwrap().is_empty());
 assert_eq!(result["status"],"partial");
 assert!(!executed.iter().any(|(n,_)|n=="create_note_relative_to_view"));
 assert!(result["reply"].as_str().unwrap().contains("old viewpoint"));
}
