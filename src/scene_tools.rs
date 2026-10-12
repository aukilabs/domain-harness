//! Typed, scene-bound metadata operations. Models cannot choose storage paths or frames.
use anyhow::{Result,ensure};
use serde::Deserialize;
use serde_json::{Value,json};
pub fn volume_corner(v:&crate::semantic_volumes::Volume,camera:[f64;3],corner:&str)->Result<[f64;3]>{
 ensure!(camera.iter().chain(v.center.iter()).chain(v.size.iter()).all(|n|n.is_finite())&&v.yaw.is_finite()&&v.size.iter().all(|n|*n>0.),"Invalid corner geometry");
 ensure!(matches!(corner,"far"|"near"),"Corner must be far or near");
 let (sin,cos)=v.yaw.sin_cos();
 let x=v.size[0]/2.-0.1_f64.min(v.size[0]/4.);let z=v.size[2]/2.-0.1_f64.min(v.size[2]/4.);
 let mut candidates=Vec::new();for sx in [-1.,1.]{for sz in [-1.,1.]{let p=[v.center[0]+cos*sx*x+sin*sz*z,v.center[1]-v.size[1]/2.,v.center[2]-sin*sx*x+cos*sz*z];let d=(p[0]-camera[0]).hypot(p[2]-camera[2]);candidates.push((d,p));}}
 candidates.sort_by(|a,b|a.0.total_cmp(&b.0));
 Ok(if corner=="far"{candidates.last().unwrap().1}else{candidates.first().unwrap().1})
}
pub fn volume_corner_in_view(v:&crate::semantic_volumes::Volume,camera:[f64;3],right:[f64;3],corner:&str)->Result<[f64;3]>{
 if matches!(corner,"far"|"near"){return volume_corner(v,camera,corner);}
 ensure!(matches!(corner,"left"|"right")&&right.iter().all(|n|n.is_finite())&&right[0].hypot(right[2])>1e-6,"Invalid side corner");
 // Validate shared geometry before deriving all four inset bottom corners.
 volume_corner(v,camera,"far")?;
 let (sin,cos)=v.yaw.sin_cos();let x=v.size[0]/2.-0.1_f64.min(v.size[0]/4.);let z=v.size[2]/2.-0.1_f64.min(v.size[2]/4.);
 let sign=if corner=="right"{1.}else{-1.};let mut points=Vec::new();
 for sx in [-1.,1.]{for sz in [-1.,1.]{let p=[v.center[0]+cos*sx*x+sin*sz*z,v.center[1]-v.size[1]/2.,v.center[2]-sin*sx*x+cos*sz*z];points.push((sign*((p[0]-v.center[0])*right[0]+(p[2]-v.center[2])*right[2]),(p[0]-camera[0]).hypot(p[2]-camera[2]),p));}}
 points.sort_by(|a,b|a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));Ok(points.last().unwrap().2)
}
pub fn requests_all_volume_deletion(text:&str)->bool {
 let words=text.to_lowercase().split(|c:char|!c.is_alphanumeric()).filter(|w|!w.is_empty()&&!matches!(*w,"please"|"now")).map(str::to_owned).collect::<Vec<_>>().join(" ");
 ["delete","remove","erase"].iter().any(|verb|["all my volumes","all volumes","my volumes"].iter().any(|target|words==format!("{verb} {target}")))
}
pub fn requests_all_note_deletion(text:&str)->bool {
 let words=text.to_lowercase().split(|c:char|!c.is_alphanumeric()).filter(|w|!w.is_empty()&&!matches!(*w,"please"|"now")).map(str::to_owned).collect::<Vec<_>>().join(" ");
 ["delete","remove","erase"].iter().any(|verb|["all my notes","all notes","my notes"].iter().any(|target|words==format!("{verb} {target}")))
}
pub fn view_relative_position(origin:[f64;3],right:[f64;3],direction:&str,distance:f64)->Result<[f64;3]>{
 ensure!(origin.iter().chain(right.iter()).all(|v|v.is_finite())&&distance.is_finite()&&distance>=0.&&distance<=1000.,"Invalid relative placement");
 let sign=match direction{"left"=>-1.,"right"=>1.,_=>anyhow::bail!("Unsupported viewer direction")};
 let length=right[0].hypot(right[2]);ensure!(length>1e-6,"Viewer screen-left has no horizontal direction; level the view first");
 Ok([origin[0]+sign*distance*right[0]/length,origin[1],origin[2]+sign*distance*right[2]/length])
}
pub fn view_relative_depth(origin:[f64;3],camera:[f64;3],direction:&str,distance:f64)->Result<[f64;3]>{
 ensure!(origin.iter().chain(camera.iter()).all(|v|v.is_finite())&&distance.is_finite()&&distance>=0.&&distance<=1000.,"Invalid relative placement");
 let sign=match direction{"behind"=>1.,"in_front"=>-1.,_=>anyhow::bail!("Unsupported depth direction")};
 let dx=origin[0]-camera[0];let dz=origin[2]-camera[2];let length=dx.hypot(dz);
 ensure!(length>1e-6,"Camera is directly above the reference; behind/front is ambiguous");
 Ok([origin[0]+sign*distance*dx/length,origin[1],origin[2]+sign*distance*dz/length])
}
pub fn mentions_name(text:&str,name:&str)->bool{
 fn normalize(s:&str)->String{s.to_lowercase().split(|c:char|!c.is_alphanumeric()).filter(|s|!s.is_empty()).collect::<Vec<_>>().join(" ")}
 let name=normalize(name);!name.is_empty()&&format!(" {} ",normalize(text)).contains(&format!(" {name} "))
}
#[derive(Deserialize)]
#[serde(tag="kind",deny_unknown_fields)]
pub enum VolumeEdit {
 #[serde(rename="place_volume_relative_to_floor")] Floor{id:String,expected_center:[f64;3],offset_m:f64},
 #[serde(rename="set_volume_length")] Length{id:String,expected_size:[f64;3],axis:String,length:f64},
 #[serde(rename="move_volume")] Move{id:String,expected_center:[f64;3],center:[f64;3]},
 #[serde(rename="resize_volume")] Resize{id:String,expected_size:[f64;3],size:[f64;3]},
 #[serde(rename="rename_volume")] Rename{id:String,expected_name:String,name:String},
}
impl VolumeEdit {
 pub fn id(&self)->&str {match self{Self::Move{id,..}|Self::Resize{id,..}|Self::Rename{id,..}|Self::Length{id,..}|Self::Floor{id,..}=>id}}
 pub fn apply(&self,v:&mut crate::semantic_volumes::Volume)->Result<Value>{
  let before=json!(v);
  match self {
   Self::Floor{expected_center,offset_m,..}=>{ensure!(v.center==*expected_center,"Volume moved concurrently; reread it");ensure!(offset_m.is_finite(),"Invalid floor offset");v.center[1]=*offset_m+v.size[1]/2.;},
   Self::Length{expected_size,axis,length,..}=>{
    ensure!(v.size==*expected_size,"Volume resized concurrently; reread it");
    let i=match axis.as_str(){"x"=>0,"y"=>1,"z"=>2,"longest"=>(0..3).max_by(|a,b|v.size[*a].total_cmp(&v.size[*b])).unwrap(),"longest_horizontal"=>if v.size[0]>=v.size[2]{0}else{2},_=>anyhow::bail!("Axis must be x, y, z, longest, or longest_horizontal")};
    if i==1 {v.center[1]+=(*length-v.size[1])/2.;}
    v.size[i]=*length;
   },
   Self::Move{expected_center,center,..}=>{ensure!(v.center==*expected_center,"Volume moved concurrently; reread it");v.center=*center;},
   Self::Resize{expected_size,size,..}=>{ensure!(v.size==*expected_size,"Volume resized concurrently; reread it");v.center[1]+=(size[1]-v.size[1])/2.;v.size=*size;},
   Self::Rename{expected_name,name,..}=>{ensure!(v.title==*expected_name,"Volume renamed concurrently; reread it");v.title=name.trim().into();},
  }
  Ok(json!({"id":v.id,"title":v.title,"before":before,"after":v,"bottom_center":[v.center[0],v.center[1]-v.size[1]/2.,v.center[2]],"summary":format!("Updated volume {}.",v.title)}))
 }
}
#[derive(Deserialize)]
#[serde(tag="kind",deny_unknown_fields)]
pub enum NoteEdit {
 #[serde(rename="delete_all_notes")] DeleteAll{expected_ids:Vec<String>},
 #[serde(rename="create_note")] Create{name:String,description:String,position:[f64;3]},
 #[serde(rename="delete_note")] Delete{id:String,expected_name:String},
 #[serde(rename="move_note")] Move{id:String,expected_position:[f64;3],position:[f64;3]},
 #[serde(rename="rename_note")] Rename{id:String,expected_name:String,name:String},
 #[serde(rename="change_note_description")] Description{id:String,expected_description:String,description:String},
}
pub fn apply_note(notes:&mut crate::notes::Notes,edit:NoteEdit,frame:&str)->Result<Value>{
 ensure!(notes.frame==frame,"Wrong note frame");
 if let NoteEdit::DeleteAll{expected_ids}=&edit {
  notes.validate(frame)?;
  let authored=|n:&crate::notes::TextNote|n.anchor_robot.is_none()&&!n.id.starts_with("robot-name:")&&!n.id.starts_with("phone-name:");
  let ids:std::collections::BTreeSet<_>=notes.items.iter().filter(|n|authored(n)).map(|n|n.id.clone()).collect();
  let expected:std::collections::BTreeSet<_>=expected_ids.iter().cloned().collect();
  ensure!(expected.len()==expected_ids.len()&&ids==expected,"Scene notes changed; inspect again before deleting all");
  let before=notes.items.iter().filter(|n|authored(n)).map(note_summary).collect::<Vec<_>>();
  notes.items.retain(|n|!authored(n));
  return Ok(json!({"tool":"delete_all_notes","deleted_count":before.len(),"before":before,"after":[],"summary":if before.is_empty(){"No authored notes remain in the selected scene.".to_string()}else{format!("Deleted all {} authored notes in the selected scene.",before.len())}}));
 }
 let id=match &edit{NoteEdit::Create{..}=>uuid::Uuid::new_v4().to_string(),NoteEdit::Delete{id,..}|NoteEdit::Move{id,..}|NoteEdit::Rename{id,..}|NoteEdit::Description{id,..}=>id.clone(),NoteEdit::DeleteAll{..}=>unreachable!()};
 if let NoteEdit::Create{name,description,position}=&edit {
  ensure!(position.iter().all(|v|v.is_finite()&&v.abs()<100000.),"Invalid position");
  notes.items.push(crate::notes::TextNote{id:id.clone(),text:name.trim().into(),details:crate::notes::NoteDetails{title:name.trim().into(),description:description.clone(),..Default::default()},position:*position,anchor_portal:String::new(),anchor_robot:None,height_m:0.,anchor_surface:Some(crate::notes::SurfaceAnchor{frame:frame.into(),voxel_center:*position,beam_diameter_m:0.,floor:false})});
  notes.validate(frame)?;
  return Ok(json!({"id":id,"title":name,"after":notes.items.last().map(note_summary),"summary":format!("Created note {name}.")}));
 }
 let index=notes.items.iter().position(|n|n.id==id).ok_or_else(||anyhow::anyhow!("Note not found in selected scene"))?;
 let n=&mut notes.items[index];ensure!(n.anchor_robot.is_none(),"Live device labels cannot be edited");
 let before=note_summary(n);let title=if n.details.title.is_empty(){n.text.clone()}else{n.details.title.clone()};
 let deleting=matches!(&edit,NoteEdit::Delete{..});
 match edit {
  NoteEdit::Move{expected_position,position,..}=>{ensure!(n.position==expected_position,"Note moved concurrently; reread it");ensure!(position.iter().all(|v|v.is_finite()&&v.abs()<100000.),"Invalid position");n.position=position;n.height_m=0.;n.anchor_portal.clear();n.anchor_surface=Some(crate::notes::SurfaceAnchor{frame:frame.into(),voxel_center:position,beam_diameter_m:0.,floor:false});},
  NoteEdit::Rename{expected_name,name,..}=>{ensure!(title==expected_name,"Note renamed concurrently; reread it");n.text=name.trim().into();n.details.title=n.text.clone();},
  NoteEdit::Description{expected_description,description,..}=>{ensure!(n.details.description==expected_description,"Note description changed concurrently; reread it");n.details.description=description;},
  NoteEdit::Delete{expected_name,..}=>{ensure!(title==expected_name,"Note renamed concurrently; reread it");notes.items.remove(index);},
  NoteEdit::Create{..}|NoteEdit::DeleteAll{..}=>unreachable!(),
 }
 notes.validate(frame)?;
 Ok(json!({"id":id,"title":title,"before":before,"after":notes.items.iter().find(|n|n.id==id).map(note_summary),"summary":format!("{} note {title}.",if deleting{"Deleted"}else{"Updated"})}))
}
fn note_summary(n:&crate::notes::TextNote)->Value{json!({"id":n.id,"name":if n.details.title.is_empty(){&n.text}else{&n.details.title},"position":n.position,"description":n.details.description})}
pub fn decode<T:serde::de::DeserializeOwned>(name:&str,mut args:Value)->Result<T>{
 let object=args.as_object_mut().ok_or_else(||anyhow::anyhow!("Tool arguments must be an object"))?;
 ensure!(!object.contains_key("kind"),"Unexpected kind argument");object.insert("kind".into(),json!(name));Ok(serde_json::from_value(args)?)
}
pub fn extra_tools()->Vec<Value>{
 let mut tools=crate::scene_geometry::tools();
 tools.push(json!({"type":"function","function":{"name":"create_note_at_volume_corner","description":"Create a note inside a named volume's far/near ground-level corner from the captured viewer viewpoint. Home computes the corner using current center, size, yaw and camera position, inset up to 10cm inside the box. 'far corner of office' means corner farthest horizontally from the viewer; do not ask which direction. Resolve volume_id from scene. Preserves all volume metadata. No coordinates needed.","parameters":{"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"},"volume_id":{"type":"string"},"corner":{"type":"string","enum":["far","near"]}},"required":["name","description","volume_id","corner"],"additionalProperties":false}}}));
 tools.push(json!({"type":"function","function":{"name":"set_peer_alias","description":"Assign a persistent Home-local alias to a cluster peer by exact peer_id from inspect_scenegraph. Does not change transport identity or the peer's own name. Empty alias removes it. Resolve later user references using aliases; ask if ambiguous. Only change aliases when requested.","parameters":{"type":"object","properties":{"peer_id":{"type":"string"},"alias":{"type":"string","maxLength":64}},"required":["peer_id","alias"],"additionalProperties":false}}}));
 tools.push(json!({"type":"function","function":{"name":"create_volume","description":"Create a new semantic box in the selected scene. Home generates its ID and uses semantic_type=region. For a name-only request omit center/size/yaw: default is a 1-meter cube resting on reference floor Y=0 at X=Z=0, center [0,0.5,0], yaw=0. No extra questions needed. Optional description defaults empty. Only supply geometry when requested; XYZ meters, center position and yaw radians. Does not overwrite existing volumes. Do not substitute creation for moving an existing volume.","parameters":{"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"},"center":{"type":"array","items":{"type":"number"},"minItems":3,"maxItems":3},"size":{"type":"array","items":{"type":"number"},"minItems":3,"maxItems":3},"yaw":{"type":"number"}},"required":["name"],"additionalProperties":false}}}));
 tools.push(json!({"type":"function","function":{"name":"delete_all_notes","description":"Atomically delete ALL authored notes in selected scene for explicit 'delete all my notes', 'delete all notes' or 'delete my notes'. Use instead of individual deletes. Pass every current authored note ID from inspect_scenegraph as expected_ids. Exclude live phone/robot labels (authored=false). Preserves volumes, voxels and other scenes. Empty list is valid.","parameters":{"type":"object","properties":{"expected_ids":{"type":"array","items":{"type":"string"},"uniqueItems":true}},"required":["expected_ids"],"additionalProperties":false}}}));
 tools.push(json!({"type":"function","function":{"name":"delete_all_volumes","description":"Atomically delete ALL semantic volumes in the selected scene when explicitly asked 'delete all my volumes', 'delete all volumes' or 'delete my volumes'. Use this instead of individual deletes. Pass all current volume IDs from inspect_scenegraph as expected_ids. Preserves notes, voxels, anchors and transforms. Empty list is valid if none remain.","parameters":{"type":"object","properties":{"expected_ids":{"type":"array","items":{"type":"string"},"uniqueItems":true}},"required":["expected_ids"],"additionalProperties":false}}}));
 tools.push(json!({"type":"function","function":{"name":"create_note_relative_to_view","description":"Create a note a specified number of meters left/right/behind/in front of a reference object from the user's captured viewer perspective. Home calculates horizontal placement from current reference center/point; preserves its elevation. Behind means away from the viewer through the reference; in_front means toward the viewer. Resolve reference ID from scene; never invent coordinates. For a peer alias, resolve its peer_id from inventory, use reference_kind=peer and reference_id=peer_id; requires a current localized position in selected scene.","parameters":{"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"},"reference_id":{"type":"string"},"reference_kind":{"type":"string","enum":["volume","note","anchor","peer"]},"direction":{"type":"string","enum":["left","right","behind","in_front"]},"distance_m":{"type":"number","minimum":0}},"required":["name","description","reference_id","reference_kind","direction","distance_m"],"additionalProperties":false}}}));
 tools.push(json!({"type":"function","function":{"name":"clear_voxels","description":"Clear occupied and free voxel evidence ONLY in the selected Home-owned local scene, preserving notes, volumes, anchors and localization transforms. Use only when the user explicitly asks to clear/reset/delete/remove the voxels or voxel map. Does not stop scanning; new observations can repopulate it. Cannot clear remote peers' private maps.","parameters":{"type":"object","properties":{},"additionalProperties":false}}}));
 tools.push(json!({"type":"function","function":{"name":"place_volume_relative_to_floor","description":"ABSOLUTE placement relative to Home's reference floor Y=0 (not measured physical floor). Place volume BOTTOM at signed offset_m: '1 meter below the floor' => offset_m=-1, regardless of current height. Home computes center Y from bottom + half height. 'Above' is positive. Preserves X/Z, all dimensions, title, description, orientation and ID; never creates a volume. Read current center first.","parameters":{"type":"object","properties":{"id":{"type":"string"},"expected_center":{"type":"array","items":{"type":"number"},"minItems":3,"maxItems":3},"offset_m":{"type":"number"}},"required":["id","expected_center","offset_m"],"additionalProperties":false}}}));
 tools.push(json!({"type":"function","function":{"name":"set_volume_length","description":"Set ONE dimension to a specified length, preserving every other dimension and orientation. Height (Y) changes preserve bottom elevation by shifting center Y automatically; X/Z changes preserve center. Never separately move the center to compensate. Preferred for 'make woohoo 15 meters long in its longest axis': use axis='longest', length=15. Home determines the longest current axis itself. Use longest_horizontal only when explicitly horizontal. Read current size first for expected_size.","parameters":{"type":"object","properties":{"id":{"type":"string"},"expected_size":{"type":"array","items":{"type":"number"},"minItems":3,"maxItems":3},"axis":{"type":"string","enum":["x","y","z","longest","longest_horizontal"]},"length":{"type":"number"}},"required":["id","expected_size","axis","length"],"additionalProperties":false}}}));
 for (name,description,fields) in [
 ("move_volume","Set volume CENTER XYZ in selected scene meters; preserve size and orientation. Look up current center yourself. For lift/raise by H, add H meters to Y; for lower subtract H. Keep X/Z unchanged. Relative moves do not require the user to provide absolute coordinates.",vec![("id","string"),("expected_center","vector"),("center","vector")]),
 ("resize_volume","Set volume dimensions XYZ in meters. Preserve bottom elevation: Home shifts center Y by half the height difference automatically, so growth goes upward and shrinking lowers the top. Preserve center X/Z and orientation. Never add a second move to compensate for height changes.",vec![("id","string"),("expected_size","vector"),("size","vector")]),
 ("rename_volume","Set volume title; preserve all other metadata.",vec![("id","string"),("expected_name","string"),("name","string")]),
 ("delete_volume","Delete the specific semantic volume explicitly requested by the user, by exact ID in selected scene. Call get_semantic_volume first and pass its current title as expected_name. Never delete a note or substitute another volume for a missing or ambiguous target.",vec![("id","string"),("expected_name","string")]),
 ("get_note","Read an authored note by exact ID, including current position, name and description.",vec![("id","string")]),
 ("create_note","Create a positioned note in selected scene meters. Resolve location from scene or explicit XYZ, never guess. description may be empty.",vec![("name","string"),("description","string"),("position","vector")]),
 ("delete_note","Delete the specific authored note explicitly requested by the user. Read it first; never delete live device labels.",vec![("id","string"),("expected_name","string")]),
 ("move_note","Set note position XYZ in selected scene meters; preserve name and description.",vec![("id","string"),("expected_position","vector"),("position","vector")]),
 ("rename_note","Replace note name, preserving its description and position.",vec![("id","string"),("expected_name","string"),("name","string")]),
 ("change_note_description","Replace entire note description; preserve name and position. For append requests include existing text.",vec![("id","string"),("expected_description","string"),("description","string")]),
 ] {
  let mut properties=serde_json::Map::new();let mut required=Vec::new();for (key,kind) in fields{required.push(key);properties.insert(key.into(),if kind=="vector"{json!({"type":"array","items":{"type":"number"},"minItems":3,"maxItems":3})}else{json!({"type":"string"})});}
  tools.push(json!({"type":"function","function":{"name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}}));
 }
 tools
}
