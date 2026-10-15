//! Exercise the actual HTTP handlers/shared dispatcher in process. No listener,
//! model, live peer, device command, or user scene data is involved.
use super::*;

fn state(dir:&std::path::Path)->anyhow::Result<Shared>{
    let mut store=Store::open_canonical(dir.join("home.json"))?;
    spatial_home::notes::initialize(&mut store)?;
    Ok(Arc::new(AppState{
        local_scenes:Arc::new(spatial_home::local_scenes::LocalScenes::load(&dir.join("scenes"))?),
        simulation_disconnect:Default::default(),simulation:None,
        fiducials:Mutex::new(spatial_home::fiducials::FiducialStore::load(dir.join("fiducials.json"))?),
        picks:Mutex::new(Default::default()),occupancy:Default::default(),ar_frames:Default::default(),
        robot_states:Default::default(),
        previews:tokio::sync::mpsc::channel(1).0,controls:tokio::sync::mpsc::channel(1).0,
        pointing:tokio::sync::mpsc::channel(1).0,
        navigation:tokio::sync::mpsc::channel(1).0,
        greetings:Mutex::new(Default::default()),
        store:Arc::new(Mutex::new(store)),changes:tokio::sync::watch::channel(()).0,
        peer:tokio::sync::watch::channel(json!({"peers":[]})).1,
        runner:tokio::sync::watch::channel(Default::default()).1,
        mapping:tokio::sync::watch::channel(json!({})).1,
        remote_scene:tokio::sync::watch::channel(Default::default()).1,
        home_live:tokio::sync::watch::channel(Default::default()).1,runners:Default::default(),
    }))
}
fn request(command:&str,args:Value)->spatial_home::commands::Request{
    serde_json::from_value(json!({"scene":"demo","frame":spatial_home::model::FRAME,"command":command,"args":args,
        "viewer":{"frame":spatial_home::model::FRAME,"mode":"3d","screen_right":[1,0,0]}})).unwrap()
}
fn headers()->HeaderMap{let mut h=HeaderMap::new();h.insert(header::HOST,"127.0.0.1:4329".parse().unwrap());h}

#[tokio::test]
async fn direct_notes_use_the_same_dispatcher_as_chat_and_preserve_write_guard()->anyhow::Result<()> {
    let dir=tempfile::tempdir()?;let s=state(dir.path())?;
    let args=json!([{"type":"text","value":"cheese"},{"type":"number","value":1},{"type":"number","value":2},{"type":"number","value":3}]);
    assert!(execute_command(State(s.clone()),HeaderMap::new(),Json(request("note",args.clone()))).await.is_err());
    let result=execute_command(State(s.clone()),headers(),Json(request("note",args))).await.map_err(|e|e.0)?.0;
    let id=result["changes"][0]["id"].as_str().unwrap();
    let context=command_context(&s,"demo".into(),None).map_err(|e|e.0)?;
    let read=scene_tool(s.clone(),context.clone(),"get_note".into(),json!({"id":id})).await?;
    assert_eq!(read["name"],"cheese");assert_eq!(read["position"],json!([1.,2.,3.]));
    let result=execute_command(State(s.clone()),headers(),Json(request("rename",json!([{"type":"entity","kind":"note","id":id},{"type":"text","value":"Cheddar"}])))).await.map_err(|e|e.0)?.0;
    assert_eq!(result["changes"].as_array().unwrap().len(),1);
    assert_eq!(scene_tool(s.clone(),context,"get_note".into(),json!({"id":id})).await?["name"],"Cheddar");
    let inv=get_commands(State(s.clone()),axum::extract::Query(VolumeQuery{scene:"demo".into()})).await.map_err(|e|e.0)?.0;
    assert!(inv["entities"].as_array().unwrap().iter().any(|e|e["id"]==id));
    let mut stale=request("note",json!([]));stale.frame="another-origin".into();
    assert!(execute_command(State(s),headers(),Json(stale)).await.is_err());
    Ok(())
}

#[tokio::test]
async fn path_command_persists_the_same_paths_read_by_home_and_annotator()->anyhow::Result<()> {
    let dir=tempfile::tempdir()?;let s=state(dir.path())?;
    let (key,snapshot)=s.local_scenes.snapshots.first_key_value().unwrap();
    let frame=snapshot.scenegraph.map.frame.id.clone();let scene=format!("local:{key}");
    let mut ids=Vec::new();
    for (name,x) in [("Kitchen",0.05),("Living room",2.05),("Bedroom",-1.05)] {
        let receipt=s.local_scenes.volumes.create_volume(key,&frame,name,"",Some([x,1.,0.05]),Some([1.,1.,1.]),Some(0.))?;
        ids.push(receipt["id"].as_str().unwrap().to_owned());
    }
    let args=json!([{"type":"entity","kind":"volume","id":ids[0]},{"type":"entity","kind":"volume","id":ids[1]},{"type":"entity","kind":"volume","id":ids[2]}]);
    let make_request=||serde_json::from_value(json!({"scene":scene,"frame":frame,"command":"path","args":args,"viewer":{"frame":frame,"mode":"3d","screen_right":[1,0,0]}})).unwrap();
    assert!(execute_command(State(s.clone()),HeaderMap::new(),Json(make_request())).await.is_err());
    assert!(s.local_scenes.paths.get(key,&frame)?.items.is_empty());
    let result=execute_command(State(s.clone()),headers(),Json(make_request())).await.map_err(|e|e.0)?.0;
    assert_eq!(result["changes"].as_array().unwrap().len(),1);
    let receipt=&result["changes"][0];let saved=s.local_scenes.paths.get(key,&frame)?;
    assert_eq!(json!(saved.items[0]),receipt["after"]);assert_eq!(receipt["frame"],frame);
    assert_eq!(saved.items[0].color,"#ff5fa8");assert_eq!(saved.items[0].segments[0],vec![[0.05,0.06,0.05],[2.05,0.06,0.05],[-1.05,0.06,0.05]]);
    assert_eq!(receipt["waypoints"].as_array().unwrap().len(),3);
    assert!(receipt["summary"].as_str().unwrap().contains("unobserved"));
    assert_eq!(s.local_scenes.view(key)?["paths"],json!(saved));
    let reloaded=spatial_home::local_scenes::LocalScenes::load(&dir.path().join("scenes"))?;
    assert_eq!(reloaded.paths.get(key,&frame)?,saved);
    let mut wrong=make_request();wrong.frame="other".into();
    assert!(execute_command(State(s.clone()),headers(),Json(wrong)).await.is_err());
    assert_eq!(s.local_scenes.paths.get(key,&frame)?,saved);
    // Shared dispatcher rejects an unavailable peer without persisting anything.
    let context=command_context(&s,scene.clone(),None).map_err(|e|e.0)?;
    assert!(scene_tool(s.clone(),context,"create_path".into(),json!({"waypoints":[{"kind":"peer","id":"missing"},{"kind":"volume","id":ids[1]}]})).await.is_err());
    assert_eq!(s.local_scenes.paths.get(key,&frame)?,saved);
    let far=s.local_scenes.volumes.create_volume(key,&frame,"Too far","",Some([100.,1.,0.05]),Some([1.,1.,1.]),Some(0.))?;
    let mut bad=make_request();bad.args[2]=spatial_home::commands::Argument::Entity{kind:"volume".into(),id:far["id"].as_str().unwrap().into()};
    let error=execute_command(State(s.clone()),headers(),Json(bad)).await.err().unwrap().0.to_string();
    assert!(error.contains("Leg 2"));assert_eq!(s.local_scenes.paths.get(key,&frame)?,saved);
    Ok(())
}
