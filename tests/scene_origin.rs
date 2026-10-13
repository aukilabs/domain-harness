mod support;
use anyhow::Result;
use auki_scenegraph::{MapSnapshot,RigidTransform};
use spatial_home::{local_scenes::LocalScenes,scene_origin::{self,Observation}};
use std::{sync::Arc,time::Instant};

fn offer(scene:&MapSnapshot,code:&str)->Observation {
    Observation{expected_scene:scene.clone(),code:code.into(),side_length_m:0.15,timestamp_ns:10,
        portal_to_native:RigidTransform{from_frame_id:format!("portal:{code}:local"),to_frame_id:format!("phone-ar:{}",uuid::Uuid::new_v4()),translation:[1.,2.,3.],rotation_wxyz:[std::f64::consts::FRAC_1_SQRT_2,-std::f64::consts::FRAC_1_SQRT_2,0.,0.]}}
}

#[test]
fn first_observation_is_origin_and_reset_invalidates_old_offers()->Result<()> {
    let dir=tempfile::tempdir()?;let scenes=LocalScenes::load(dir.path())?;
    let empty=scenes.snapshot("humanoid-hub-conference")?;
    assert!(empty.scenegraph.anchors.is_empty());
    assert_eq!(scene_origin::portal(empty)?,None);
    let observation=offer(empty,"ABCDEFGHIJK");
    let established=scene_origin::establish(empty,&observation)?;
    let mut wall=observation.clone();wall.portal_to_native.rotation_wxyz=[1.,0.,0.,0.];
    assert!(scene_origin::establish(empty,&wall).unwrap_err().to_string().contains("First Portal"));
    let mut unknown=observation.clone();unknown.portal_to_native.to_frame_id="camera".into();
    assert!(scene_origin::establish(empty,&unknown).unwrap_err().to_string().contains("gravity"));
    assert_eq!(scene_origin::portal(&established)?,Some("ABCDEFGHIJK"));
    let anchor=&established.scenegraph.anchors["ABCDEFGHIJK"];
    assert_eq!(anchor.pose_in_map.translation,[0.;3]);
    assert_eq!(anchor.pose_in_map.rotation_wxyz,[std::f64::consts::FRAC_1_SQRT_2,-std::f64::consts::FRAC_1_SQRT_2,0.,0.]);
    assert!(scene_origin::establish(&established,&offer(empty,"ZYXWVUTSRQP")).is_err());
    let reset=scene_origin::reset(&established)?;
    assert_eq!(reset.scenegraph.map.frame.id,empty.scenegraph.map.frame.id);
    assert!(scene_origin::establish(&reset,&observation).is_err());
    let next=scene_origin::establish(&reset,&offer(&reset,"ZYXWVUTSRQP"))?;
    assert_eq!(scene_origin::portal(&next)?,Some("ZYXWVUTSRQP"));
    for bad in ["bad","ABCDEFGHIJKL","abcdefghijk"] {assert!(scene_origin::establish(empty,&offer(empty,bad)).is_err());}
    let mut bad=observation.clone();bad.timestamp_ns=0;assert!(scene_origin::establish(empty,&bad).is_err());
    bad=observation.clone();bad.portal_to_native.rotation_wxyz=[0.;4];assert!(scene_origin::establish(empty,&bad).is_err());
    bad=observation.clone();bad.portal_to_native.from_frame_id="different-marker".into();assert!(scene_origin::establish(empty,&bad).is_err());
    Ok(())
}

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn sdk_first_writer_wins_and_reset_preserves_authored_content()->Result<()> {
    let pair=support::LocalPair::start().await?;
    let result=async {
        let dir=tempfile::tempdir()?;let scenes=Arc::new(LocalScenes::load(dir.path())?);
        let server=spatial_home::network::ScenePublisher::mount(&pair.a)?;
        let owner=pair.a.peer_id().to_string();let clock=spatial_home::model::clock_entry(&owner,&uuid::Uuid::new_v4().to_string());
        let mut publishers=spatial_home::local_mobile::Publisher::mount_all(&server.runtime,&server.endpoint,scenes.clone(),clock,Instant::now())?;
        let home=scenes.connection("humanoid-hub-conference",spatial_home::network::HomeConnection::direct(&pair.a))?;
        let editor=scenes.note_editors.lock().unwrap()["humanoid-hub-conference"].clone();
        let empty=editor.reference()?;
        let first=offer(&empty,"ABCDEFGHIJK");let other=offer(&empty,"ZYXWVUTSRQP");
        let mut wall=first.clone();wall.portal_to_native.rotation_wxyz=[1.,0.,0.,0.];
        assert!(scene_origin::send(&pair.b,&home,&wall).await.is_err());
        assert!(editor.reference()?.scenegraph.anchors.is_empty(),"Rejected wall must not establish an origin");
        let (a,b)=tokio::join!(scene_origin::send(&pair.b,&home,&first),scene_origin::send(&pair.b,&home,&other));
        assert_eq!([a.is_ok(),b.is_ok()].into_iter().filter(|x|*x).count(),1);
        let saved=editor.reference()?;assert_eq!(saved.scenegraph.anchors.len(),1);
        assert_eq!(LocalScenes::load(dir.path())?.snapshot("humanoid-hub-conference")?,&saved);
        // Exercise the same SDK subscription used by a phone: the accepted
        // reference must actually leave Home, not merely update server memory.
        // Home's publication tick is intentionally capped at 1 Hz.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        for publisher in &mut publishers { publisher.tick()?; }
        let stop=tokio_util::sync::CancellationToken::new();
        let mut published=None;
        tokio::time::timeout(std::time::Duration::from_secs(5),
            spatial_home::mobile::receive_portals(&pair.b,&home,&stop,std::time::Duration::from_secs(5),|value| {
                published=Some(value);stop.cancel();
            })).await??;
        let published=published.expect("Home must publish its accepted origin");
        assert_eq!(published["scenegraph"]["anchors"],serde_json::to_value(&saved.scenegraph.anchors)?);
        assert_eq!(published["scenegraph"]["map"]["frame"]["id"],home.frame);
        let receipt=editor.tool_edit(spatial_home::scene_tools::NoteEdit::Create{name:"keep me".into(),description:"unchanged".into(),position:[1.,2.,3.]})?;
        let note_id=receipt["id"].as_str().unwrap();let note_before=editor.get_note(note_id)?;
        let volume=spatial_home::semantic_volumes::Volume{id:uuid::Uuid::new_v4().to_string(),title:"keep volume".into(),description:"unchanged".into(),semantic_type:"region".into(),center:[1.,1.,1.],size:[2.;3],yaw:0.};
        scenes.volumes.change("humanoid-hub-conference",&home.frame,None,Some(volume),&home.frame)?;
        let volumes_before=serde_json::to_value(scenes.volumes.get("humanoid-hub-conference",&home.frame)?)?;
        assert!(editor.reset_mapping(&empty).is_err(),"reset must reject a stale reference");
        let receipt=editor.reset_mapping(&saved)?;assert_eq!(receipt["voxels_cleared"],true);
        assert_eq!(editor.get_note(note_id)?,note_before);
        assert_eq!(serde_json::to_value(scenes.volumes.get("humanoid-hub-conference",&home.frame)?)?,volumes_before);
        let reset=editor.reference()?;assert!(reset.scenegraph.anchors.is_empty());
        assert!(scene_origin::send(&pair.b,&home,&first).await.is_err());
        let fresh=offer(&reset,"NEWPORTAL01");assert!(scene_origin::send(&pair.b,&home,&fresh).await?);
        assert_eq!(scene_origin::portal(&editor.reference()?)?,Some("NEWPORTAL01"));
        Ok::<(),anyhow::Error>(())
    }.await;
    pair.shutdown().await?;result
}
