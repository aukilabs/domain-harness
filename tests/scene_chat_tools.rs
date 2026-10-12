use spatial_home::{scene_tools::*,semantic_volumes::{Storage,Volume},notes::Notes};
#[test]
fn far_corner_tracks_viewpoint_rotation_and_bottom()->anyhow::Result<()>{
 let mut v=Volume{id:uuid::Uuid::new_v4().to_string(),title:"office".into(),description:"".into(),semantic_type:"region".into(),center:[0.,1.,0.],size:[10.,2.,10.],yaw:0.};
 assert_eq!(volume_corner(&v,[10.,5.,10.],"far")?,[-4.9,0.,-4.9]);
 assert_eq!(volume_corner(&v,[-10.,5.,-10.],"far")?,[4.9,0.,4.9]);
 assert_eq!(volume_corner(&v,[10.,5.,10.],"near")?,[4.9,0.,4.9]);
 v.yaw=0.4;v.center[1]=-0.7;v.size=[4.,0.05,2.];let p=volume_corner(&v,[8.,4.,7.],"far")?;
 assert!((p[1]+0.725).abs()<1e-12);let (sin,cos)=v.yaw.sin_cos();let x=cos*p[0]-sin*p[2];let z=sin*p[0]+cos*p[2];assert!((x.abs()-1.9).abs()<1e-12);assert!((z.abs()-0.9).abs()<1e-12);Ok(())
}
#[test]
fn height_edits_preserve_bottom_even_below_reference_zero()->anyhow::Result<()>{
 let mut v=Volume{id:uuid::Uuid::new_v4().to_string(),title:"office".into(),description:"".into(),semantic_type:"region".into(),center:[0.,-0.7,0.],size:[10.,0.05,10.],yaw:0.2};
 let base=v.center[1]-v.size[1]/2.;
 VolumeEdit::Length{id:v.id.clone(),expected_size:v.size,axis:"y".into(),length:2.}.apply(&mut v)?;
 assert!((v.center[1]-v.size[1]/2.-base).abs()<1e-12);assert!((v.center[1]-0.275).abs()<1e-12);assert_eq!(v.size,[10.,2.,10.]);
 VolumeEdit::Resize{id:v.id.clone(),expected_size:v.size,size:[5.,0.1,5.]}.apply(&mut v)?;
 assert!((v.center[1]-v.size[1]/2.-base).abs()<1e-12);assert_eq!(v.center[0],0.);assert_eq!(v.yaw,0.2);Ok(())
}
#[test]
fn create_volume_defaults_persist_and_duplicate_names_do_not_overwrite()->anyhow::Result<()>{
 let dir=tempfile::tempdir()?;let store=Storage::new(dir.path().into());
 let receipt=store.create_volume("room","frame","office","",None,None,None)?;
 let saved=Storage::new(dir.path().into()).get("room","frame")?.items.remove(0);
 assert_eq!(saved.id,receipt["id"].as_str().unwrap());assert_eq!(saved.title,"office");assert_eq!(saved.center,[0.,0.5,0.]);assert_eq!(saved.size,[1.;3]);assert_eq!(saved.semantic_type,"region");
 assert!(store.create_volume("room","frame","Office","",None,None,None).is_err());assert_eq!(store.get("room","frame")?.items,vec![saved]);
 assert!(store.create_volume("room","frame","bad","",None,Some([-1.;3]),None).is_err());assert_eq!(store.get("room","frame")?.items.len(),1);Ok(())
}
#[test]
fn bulk_notes_preserves_live_labels_and_rejects_partial_snapshot()->anyhow::Result<()>{
 let mut notes=Notes::empty("local-scene:test-room:portal-HNY3JGNLRAE");
 for name in ["A","B"]{apply_note(&mut notes,NoteEdit::Create{name:name.into(),description:String::new(),position:[0.;3]},"local-scene:test-room:portal-HNY3JGNLRAE")?;}
 let ids=notes.items.iter().map(|n|n.id.clone()).collect::<Vec<_>>();
 let mut live=notes.items[0].clone();live.id="phone-name:test".into();notes.items.push(live);
 assert!(apply_note(&mut notes,NoteEdit::DeleteAll{expected_ids:vec![ids[0].clone()]},"local-scene:test-room:portal-HNY3JGNLRAE").is_err());assert_eq!(notes.items.len(),3);
 let receipt=apply_note(&mut notes,NoteEdit::DeleteAll{expected_ids:ids},"local-scene:test-room:portal-HNY3JGNLRAE")?;assert_eq!(receipt["deleted_count"],2);assert_eq!(notes.items.len(),1);assert_eq!(notes.items[0].id,"phone-name:test");
 assert_eq!(apply_note(&mut notes,NoteEdit::DeleteAll{expected_ids:vec![]},"local-scene:test-room:portal-HNY3JGNLRAE")?["deleted_count"],0);
 assert!(requests_all_note_deletion("delete all my notes"));assert!(!requests_all_note_deletion("don't delete all notes"));Ok(())
}
#[test]
fn bulk_delete_is_atomic_scoped_and_handles_empty_scene()->anyhow::Result<()>{
 let dir=tempfile::tempdir()?;let store=Storage::new(dir.path().into());
 let v=Volume{id:uuid::Uuid::new_v4().to_string(),title:"A".into(),description:"".into(),semantic_type:"region".into(),center:[0.;3],size:[1.;3],yaw:0.};
 let mut b=v.clone();b.id=uuid::Uuid::new_v4().to_string();b.title="B".into();
 for item in [&v,&b]{store.change("room","frame",None,Some(item.clone()),"frame")?;}
 store.change("other","frame",None,Some(v.clone()),"frame")?;
 assert!(store.delete_all_volumes("room","frame",&[v.id.clone()]).is_err());assert_eq!(store.get("room","frame")?.items.len(),2);
 let result=store.delete_all_volumes("room","frame",&[b.id.clone(),v.id.clone()])?;assert_eq!(result["deleted_count"],2);
 assert!(Storage::new(dir.path().into()).get("room","frame")?.items.is_empty());assert_eq!(store.get("other","frame")?.items.len(),1);
 assert_eq!(store.delete_all_volumes("room","frame",&[])?["deleted_count"],0);
 for text in ["delete all my volumes","Delete all volumes","delete my volumes"]{assert!(requests_all_volume_deletion(text));}
 for text in ["delete Office","don't delete all my volumes","delete all my volumes except Office"]{assert!(!requests_all_volume_deletion(text));}
 Ok(())
}
#[test]
fn volume_deletion_is_persisted_and_scoped()->anyhow::Result<()>{
 let dir=tempfile::tempdir()?;let store=Storage::new(dir.path().into());
 let v=Volume{id:uuid::Uuid::new_v4().to_string(),title:"Delete me".into(),description:"Rules".into(),semantic_type:"region".into(),center:[0.,1.,0.],size:[2.;3],yaw:0.};
 let mut keep=v.clone();keep.id=uuid::Uuid::new_v4().to_string();keep.title="Keep me".into();
 store.change("room","frame",None,Some(v.clone()),"frame")?;store.change("room","frame",None,Some(keep.clone()),"frame")?;
 assert!(store.delete_volume("other","frame",&v.id,&v.title).is_err());
 assert!(store.delete_volume("room","frame",&v.id,"Old name").is_err());
 let receipt=store.delete_volume("room","frame",&v.id,&v.title)?;assert!(receipt["after"].is_null());
 assert_eq!(Storage::new(dir.path().into()).get("room","frame")?.items,vec![keep]);
 assert!(store.delete_volume("room","frame",&v.id,&v.title).is_err());Ok(())
}
#[test]
fn behind_places_reference_between_camera_and_note()->anyhow::Result<()>{
 let reference=[1.,2.,3.];let camera=[4.,8.,7.];
 assert_eq!(view_relative_depth(reference,camera,"behind",5.)?,[-2.,2.,-1.]);
 assert_eq!(view_relative_depth(reference,camera,"in_front",5.)?,[4.,2.,7.]);
 assert!(view_relative_depth(reference,[1.,9.,3.],"behind",4.).is_err());Ok(())
}
#[test]
fn screen_left_uses_captured_view_and_preserves_elevation()->anyhow::Result<()>{
 assert_eq!(view_relative_position([1.,2.,3.],[1.,0.,0.],"left",4.)?,[-3.,2.,3.]);
 assert_eq!(view_relative_position([1.,2.,3.],[0.,0.,-1.],"left",4.)?,[1.,2.,7.]);
 assert_eq!(view_relative_position([1.,2.,3.],[0.,0.6,-0.8],"right",4.)?,[1.,2.,-1.]);
 assert!(view_relative_position([0.;3],[0.,1.,0.],"left",4.).is_err());
 Ok(())
}
#[test]
fn below_floor_is_absolute_and_never_creates_another_volume()->anyhow::Result<()>{
 let dir=tempfile::tempdir()?;let store=Storage::new(dir.path().into());
 let v=Volume{id:uuid::Uuid::new_v4().to_string(),title:"TMI".into(),description:"Rules".into(),semantic_type:"circle".into(),center:[1.109544,8.2680879,-1.1906646],size:[2.,1.,4.],yaw:0.3};
 store.change("room","frame",None,Some(v.clone()),"frame")?;
 store.tool_edit("room","frame",VolumeEdit::Floor{id:v.id.clone(),expected_center:v.center,offset_m:-1.})?;
 let volumes=Storage::new(dir.path().into()).get("room","frame")?;assert_eq!(volumes.items.len(),1);
 let saved=&volumes.items[0];assert_eq!(saved.id,v.id);assert_eq!(saved.center,[v.center[0],-0.5,v.center[2]]);assert_eq!(saved.center[1]-saved.size[1]/2.,-1.);assert_eq!(saved.size,v.size);assert_eq!(saved.title,v.title);assert_eq!(saved.semantic_type,v.semantic_type);assert_eq!(saved.yaw,v.yaw);Ok(())
}
#[test]
fn longest_axis_resize_is_resolved_by_storage_and_preserves_other_dimensions()->anyhow::Result<()>{
 let dir=tempfile::tempdir()?;let store=Storage::new(dir.path().into());
 let v=Volume{id:uuid::Uuid::new_v4().to_string(),title:"woohoo".into(),description:"Rules".into(),semantic_type:"region".into(),center:[1.,0.,3.],size:[1.0113,2.,0.6],yaw:0.3};
 store.change("room","frame",None,Some(v.clone()),"frame")?;
 store.tool_edit("room","frame",VolumeEdit::Length{id:v.id.clone(),expected_size:v.size,axis:"longest".into(),length:15.})?;
 let reopened=Storage::new(dir.path().into()).get("room","frame")?.items.remove(0);
 assert_eq!(reopened.size,[1.0113,15.,0.6]);assert_eq!(reopened.center,[1.,6.5,3.]);assert_eq!(reopened.yaw,v.yaw);assert_eq!(reopened.description,v.description);Ok(())
}
#[test]
fn volume_tools_preserve_other_fields_and_reject_stale_edits()->anyhow::Result<()>{
 let dir=tempfile::tempdir()?;let store=Storage::new(dir.path().into());
 let v=Volume{id:uuid::Uuid::new_v4().to_string(),title:"Room".into(),description:"Rules".into(),semantic_type:"room".into(),center:[0.,1.,0.],size:[2.,2.,2.],yaw:0.};
 store.change("scene","local-scene:test-room:portal-HNY3JGNLRAE",None,Some(v.clone()),"local-scene:test-room:portal-HNY3JGNLRAE")?;
 store.tool_edit("scene","local-scene:test-room:portal-HNY3JGNLRAE",VolumeEdit::Move{id:v.id.clone(),expected_center:v.center,center:[1.,2.,3.]})?;
 assert!(store.tool_edit("scene","local-scene:test-room:portal-HNY3JGNLRAE",VolumeEdit::Move{id:v.id.clone(),expected_center:v.center,center:[9.,9.,9.]}).is_err());
 store.tool_edit("scene","local-scene:test-room:portal-HNY3JGNLRAE",VolumeEdit::Resize{id:v.id.clone(),expected_size:v.size,size:[3.,4.,5.]})?;
 store.tool_edit("scene","local-scene:test-room:portal-HNY3JGNLRAE",VolumeEdit::Rename{id:v.id.clone(),expected_name:v.title,name:"Living room".into()})?;
 let saved=store.get("scene","local-scene:test-room:portal-HNY3JGNLRAE")?.items.remove(0);assert_eq!(saved.center,[1.,3.,3.]);assert_eq!(saved.size,[3.,4.,5.]);assert_eq!(saved.title,"Living room");assert_eq!(saved.description,"Rules");
 assert!(store.tool_edit("other","local-scene:test-room:portal-HNY3JGNLRAE",VolumeEdit::Rename{id:v.id.clone(),expected_name:"Living room".into(),name:"Wrong".into()}).is_err());
 assert!(store.tool_edit("scene","local-scene:test-room:portal-HNY3JGNLRAE",VolumeEdit::Resize{id:v.id,expected_size:saved.size,size:[-1.,2.,3.]}).is_err());
 assert_eq!(store.get("scene","local-scene:test-room:portal-HNY3JGNLRAE")?.items[0],saved);Ok(())
}
#[test]
fn note_tools_create_move_rename_describe_and_delete()->anyhow::Result<()>{
 let mut notes=Notes::empty("local-scene:test-room:portal-HNY3JGNLRAE");
 let created=apply_note(&mut notes,NoteEdit::Create{name:"Table".into(),description:"Old rules".into(),position:[1.,2.,3.]},"local-scene:test-room:portal-HNY3JGNLRAE")?;
 let id=created["id"].as_str().unwrap().to_string();
 apply_note(&mut notes,NoteEdit::Move{id:id.clone(),expected_position:[1.,2.,3.],position:[4.,5.,6.]},"local-scene:test-room:portal-HNY3JGNLRAE")?;
 assert!(apply_note(&mut notes.clone(),NoteEdit::Move{id:id.clone(),expected_position:[1.,2.,3.],position:[0.,0.,0.]},"local-scene:test-room:portal-HNY3JGNLRAE").is_err());
 apply_note(&mut notes,NoteEdit::Rename{id:id.clone(),expected_name:"Table".into(),name:"Desk".into()},"local-scene:test-room:portal-HNY3JGNLRAE")?;
 apply_note(&mut notes,NoteEdit::Description{id:id.clone(),expected_description:"Old rules".into(),description:"New rules".into()},"local-scene:test-room:portal-HNY3JGNLRAE")?;
 assert_eq!(notes.items[0].position,[4.,5.,6.]);assert_eq!(notes.items[0].text,"Desk");assert_eq!(notes.items[0].details.description,"New rules");
 assert!(apply_note(&mut notes.clone(),NoteEdit::Delete{id:id.clone(),expected_name:"Desk".into()},"other-frame").is_err());
 apply_note(&mut notes,NoteEdit::Delete{id,expected_name:"Desk".into()},"local-scene:test-room:portal-HNY3JGNLRAE")?;assert!(notes.items.is_empty());
 assert!(decode::<NoteEdit>("create_note",serde_json::json!({"name":"Bad","description":"","position":[0,0,0],"extra":true})).is_err());Ok(())
}
