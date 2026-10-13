use spatial_home::scene_paths::*;
fn stroke()->Stroke { Stroke {id:uuid::Uuid::new_v4().to_string(),title:"Route".into(),color:"#ffca55".into(),width_m:0.035,segments:vec![vec![[0.,0.,0.],[1.,0.,0.],[1.,0.,1.]]] } }
#[test]
fn paths_persist_are_idempotent_and_frame_isolated()->anyhow::Result<()> {
    let dir=tempfile::tempdir()?;let storage=Storage::new(dir.path().into());let s=stroke();
    let saved=storage.change("room","frame-a",&s.id,Some(s.clone()))?;
    assert_eq!(storage.change("room","frame-a",&s.id,Some(s.clone()))?,saved);
    assert_eq!(Storage::new(dir.path().into()).get("room","frame-a")?,saved);
    assert!(storage.get("room","frame-b").is_err());
    assert!(storage.get("../room","frame-a").is_err());
    assert!(storage.get("other","frame-a")?.items.is_empty());
    assert!(storage.change("room","frame-a",&s.id,None)?.items.is_empty());Ok(())
}
#[test]
fn malformed_paths_are_rejected(){
    let mut paths=Paths::empty("frame");paths.items.push(stroke());
    assert!(paths.validate("frame").is_ok());paths.items[0].segments[0][0][0]=f64::NAN;
    assert!(paths.validate("frame").is_err());paths.items[0]=stroke();paths.items[0].segments[0].truncate(1);
    assert!(paths.validate("frame").is_err());
}
#[test]
fn live_updates_append_idempotently_and_never_recreate_deleted_paths()->anyhow::Result<()> {
    let dir=tempfile::tempdir()?;let storage=Storage::new(dir.path().into());let mut s=stroke();
    assert!(storage.extend("room","frame",s.clone()).is_err());
    storage.change("room","frame",&s.id,Some(s.clone()))?;
    let old=s.clone();s.segments[0].push([2.,0.,1.]);
    let saved=storage.extend("room","frame",s.clone())?;
    assert_eq!(saved.items,vec![s.clone()]);
    assert_eq!(storage.extend("room","frame",s.clone())?,saved);
    assert!(storage.extend("room","frame",old).is_err());
    let mut changed=s.clone();changed.segments[0][0]=[99.,0.,0.];
    assert!(storage.extend("room","frame",changed).is_err());
    storage.change("room","frame",&s.id,None)?;
    assert!(storage.extend("room","frame",s).is_err());
    assert!(storage.get("room","frame")?.items.is_empty());Ok(())
}
#[test]
fn teleoperator_projects_paths_without_voxels_and_rejects_wrong_frame()->anyhow::Result<()> {
    let mut s=stroke();s.segments=vec![vec![[-0.1,0.,1.],[0.1,0.,1.]]];
    let paths=Paths {items:vec![s],..Paths::empty("frame")};
    let mut view=serde_json::json!({"frame":"frame","voxel_size":0.1,"voxels":[],"notes":[],"paths":paths});
    let calibration=serde_json::json!({"K":[100.,0.,50.,0.,100.,50.,0.,0.,1.],"D":[],"distortion_model":"pinhole","width":100,"height":100});
    let result=spatial_home::ar_preview::project_view(&view,&spatial_home::local_mapper::identity(),&calibration)?;
    assert_eq!(result["path_segments"].as_array().unwrap().len(),1);
    assert_eq!(result["path_segments"][0]["points"][0][0],40.);
    view["paths"]["frame"]=serde_json::json!("other");
    assert!(spatial_home::ar_preview::project_view(&view,&spatial_home::local_mapper::identity(),&calibration).is_err());Ok(())
}
