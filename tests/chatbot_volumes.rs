use spatial_home::semantic_volumes::{Storage,Volume};
#[test]
fn description_edits_preserve_geometry_reject_conflicts_and_persist()->anyhow::Result<()> {
 let dir=tempfile::tempdir()?;let storage=Storage::new(dir.path().into());
 let volume=Volume{id:uuid::Uuid::new_v4().to_string(),title:"Living room".into(),description:"Existing rule".into(),semantic_type:"room".into(),center:[1.,2.,3.],size:[2.,3.,4.],yaw:0.3};
 storage.change("room","frame",None,Some(volume.clone()),"frame")?;
 assert!(storage.update_description("other","frame",&volume.id,"Existing rule","wrong").is_err());
 assert!(storage.update_description("room","other-frame",&volume.id,"Existing rule","wrong").is_err());
 let receipt=storage.update_description("room","frame",&volume.id,"Existing rule","Existing rule\n\nDon't touch the table.")?;
 assert_eq!(receipt["before"],"Existing rule");
 let updated=storage.get("room","frame")?.items.remove(0);
 assert_eq!(updated.center,volume.center);assert_eq!(updated.size,volume.size);assert_eq!(updated.yaw,volume.yaw);assert_eq!(updated.title,volume.title);
 assert!(storage.update_description("room","frame",&volume.id,"Existing rule","stale").is_err());
 assert!(storage.update_description("room","frame",&volume.id,&updated.description,&"x".repeat(10001)).is_err());
 assert_eq!(Storage::new(dir.path().into()).get("room","frame")?.items[0],updated);Ok(())
}
