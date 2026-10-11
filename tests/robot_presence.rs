mod support;
use anyhow::Result;
use spatial_home::{robot_presence::{Presence,Sample,Update}, frame_transform::{FrameTransform,Evidence,Validity}};
use std::time::{Duration,Instant};
fn sample(owner:&str, home:&str, frame:&str) -> Sample {
    let clock = spatial_home::model::clock_ref(&spatial_home::model::clock_entry(owner,"session"));
    let dir=tempfile::tempdir().unwrap();
    let scenes=spatial_home::local_scenes::LocalScenes::load(dir.path()).unwrap();
    let reference = scenes.snapshot("humanoid-hub-conference").unwrap();
    let anchor = reference.scenegraph.anchors.values().next().unwrap().clone();
    let native = "bracketbot-local:00000000-0000-4000-8000-000000000000:zup";
    Sample { name:"Bracketbot 0184".into(),timestamp_ns:20,clock:clock.clone(),
        native_pose:auki_scenegraph::RigidTransform { from_frame_id:"robot.base".into(),to_frame_id:native.into(),translation:[1.,2.,3.],rotation_wxyz:[1.,0.,0.,0.] },
        alignment:FrameTransform {schema:spatial_home::frame_transform::SCHEMA.into(),authority_peer_id:owner.into(),
            source_frame:spatial_home::conventions::frame(native).unwrap(),
            target_frame:Some(reference.scenegraph.map.frame.clone()),validity:Validity::Valid,reason:"aligned".into(),
            transform:Some(auki_scenegraph::RigidTransform {from_frame_id:native.into(),to_frame_id:frame.into(),translation:[10.,0.,0.],rotation_wxyz:[1.,0.,0.,0.]}),
            evidence:Some(Evidence {portal:anchor.anchor_id.clone(),capture_timestamp_ns:10,clock,
                provenance:Default::default(),reference_peer_id:home.into(),reference_anchor:anchor})} }
}
#[test]
fn fresh_authenticated_robot_pose_is_transformed_and_expires() -> Result<()> {
    let dir=tempfile::tempdir()?;
    let scenes=spatial_home::local_scenes::LocalScenes::load(dir.path())?;
    let reference=scenes.snapshot("humanoid-hub-conference")?;
    let frame=&reference.scenegraph.map.frame.id;
    let now=Instant::now(); let mut presence=Presence::default();
    let s=sample("robot","home",frame);
    presence.accept("robot","home",frame,Update {sample:Some(s.clone())},now)?;
    let snapshot=presence.snapshot(now,frame);
    assert_eq!(snapshot["poses"][0]["pose"]["translation"],serde_json::json!([11.,2.,3.]));
    assert_eq!(s.native_pose.translation,[1.,2.,3.]);
    assert!(presence.snapshot(now+Duration::from_secs(3),frame)["poses"].as_array().unwrap().is_empty());
    for owner in ["other",""] { assert!(presence.accept(owner,"home",frame,Update {sample:Some(s.clone())},now).is_err()); }
    let mut stale=s.clone(); stale.alignment.validity=Validity::Stale;
    assert!(presence.accept("robot","home",frame,Update {sample:Some(stale)},now).is_err());
    assert!(presence.accept("robot","other",frame,Update {sample:Some(s)},now).is_err());
    presence.accept("robot","home",frame,Update {sample:None},now)?;
    assert!(presence.snapshot(now,frame)["poses"].as_array().unwrap().is_empty());
    Ok(())
}

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn robot_presence_arrives_without_a_geometry_contribution() -> Result<()> {
    let pair=support::LocalPair::start().await?;
    let result=async {
        let dir=tempfile::tempdir()?;
        let scenes=std::sync::Arc::new(spatial_home::local_scenes::LocalScenes::load(dir.path())?);
        let server=spatial_home::network::ScenePublisher::mount(&pair.a)?;
        let owner=pair.a.peer_id().to_string();
        let clock=spatial_home::model::clock_entry(&owner,&uuid::Uuid::new_v4().to_string());
        let mut publishers=spatial_home::local_mobile::Publisher::mount_all(&server.runtime,&server.endpoint,scenes.clone(),clock,Instant::now())?;
        let home=scenes.connection("humanoid-hub-conference",spatial_home::network::HomeConnection::direct(&pair.a))?;
        let s=sample(&pair.b.peer_id().to_string(),&owner,&home.frame);
        spatial_home::mobile_pose::send_component(&pair.b,&home,spatial_home::robot_presence::COMPONENT,&Update {sample:Some(s)}).await?;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        for publisher in &mut publishers {publisher.tick()?;}
        let mut value=None;
        spatial_home::local_mobile::receive(&pair.b,&home,&tokio_util::sync::CancellationToken::new(),Duration::from_millis(50),|v|value=Some(v)).await?;
        let value=value.unwrap();
        assert!(value["render"]["notes"].as_array().unwrap().iter().any(|n|n["text"]=="Bracketbot 0184" && n["robot_pose"].is_object()));
        assert!(value["render"]["voxels"].as_array().unwrap().is_empty());
        // Phones stay separate: robot presence must not become a reaching target.
        assert!(scenes.presence("humanoid-hub-conference")?["poses"].as_array().unwrap().is_empty());
        drop(publishers);
        server.close().await?;
        Ok(())
    }.await;
    pair.shutdown().await?;
    result
}
