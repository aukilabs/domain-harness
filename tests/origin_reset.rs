use anyhow::Result;
use spatial_home::{local_scenes::LocalScenes, origin_reset};
#[test]
fn restart_reset_preserves_authored_content_and_is_recoverable() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scene = "humanoid-hub-conference";
    let scenes = LocalScenes::load(dir.path())?;
    let snapshot = scenes.snapshot(scene)?;
    let old = &snapshot.scenegraph.map.frame.id;
    let stroke = spatial_home::scene_paths::Stroke {
        id: uuid::Uuid::new_v4().to_string(),
        title: "keep path".into(),
        color: "#ff0000".into(),
        width_m: 0.03,
        segments: vec![vec![[0., 0., 0.], [1., 0., 1.]]],
    };
    scenes
        .paths
        .change(scene, old, &stroke.id, Some(stroke.clone()))?;
    let receipt = origin_reset::stage(dir.path(), snapshot)?;
    assert_eq!(receipt["status"], "pending_restart");
    assert_eq!(
        origin_reset::stage(dir.path(), snapshot)?["new_frame"],
        receipt["new_frame"]
    );
    // Edits after scheduling are retained: content is snapshotted at startup.
    let mut later = stroke.clone();
    later.id = uuid::Uuid::new_v4().to_string();
    later.title = "later".into();
    scenes
        .paths
        .change(scene, old, &later.id, Some(later.clone()))?;
    let restarted = LocalScenes::load(dir.path())?;
    let fresh = &restarted.snapshot(scene)?.scenegraph.map.frame.id;
    assert_ne!(fresh, old);
    assert_eq!(receipt["new_frame"], *fresh);
    let base = spatial_home::network::HomeConnection {
        schema: "spatial-harness.home-connection/v1".into(), peer_id:"peer".into(),
        domain_id:"domain".into(), routes:vec![], frame:String::new(), product_id:String::new(),
    };
    let profile = restarted.connection(scene, base)?;
    assert_eq!(&profile.frame, fresh);
    assert_eq!(
        restarted.paths.get(scene, fresh)?.items,
        vec![stroke, later]
    );
    assert!(restarted.paths.get(scene, old).is_err());
    assert_eq!(
        LocalScenes::load(dir.path())?.snapshot(scene)?,
        restarted.snapshot(scene)?
    );
    // Simulate a crash after only some of the journal's replacements landed.
    let archive = std::fs::read_dir(dir.path().join(".origin-resets"))?
        .next()
        .unwrap()?
        .path();
    let journal: serde_json::Value = serde_json::from_slice(&std::fs::read(&archive)?)?;
    let first = &journal["changes"][0];
    let target = dir.path().join(first["path"].as_str().unwrap());
    std::fs::write(&target, serde_json::to_vec(&first["before"])?)?;
    std::fs::rename(&archive, archive.with_extension("pending"))?;
    origin_reset::apply_pending(dir.path())?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(target)?)?,
        first["after"]
    );
    Ok(())
}

#[test]
fn old_held_transform_cannot_target_new_origin() -> Result<()> {
    use spatial_home::compact_pose::{Contract, Lifetime};
    let dir = tempfile::tempdir()?;
    let scenes = LocalScenes::load(dir.path())?;
    let old = scenes.snapshot("humanoid-hub-conference")?;
    let fresh = spatial_home::scene_origin::reset(old)?;
    let frame = |id| auki_registry::FrameRegistryEntry::opengl("phone", id);
    let pose = Contract {
        from_frame: (&frame("camera")).into(),
        to_frame: (&frame("session")).into(),
        clock: spatial_home::model::clock_ref(&spatial_home::model::clock_entry(
            "phone", "session",
        )),
        session_id: "session".into(),
        lifetime: Lifetime::Sampled,
    };
    let held = Contract {
        from_frame: pose.to_frame.clone(),
        to_frame: (&spatial_home::conventions::registry("home", &old.scenegraph.map.frame.id)?)
            .into(),
        lifetime: Lifetime::Held,
        ..pose.clone()
    };
    spatial_home::frame_alignment::validate_chain(
        &held,
        &pose,
        "phone",
        "home",
        &old.scenegraph.map.frame.id,
    )?;
    assert!(
        spatial_home::frame_alignment::validate_chain(
            &held,
            &pose,
            "phone",
            "home",
            &fresh.scenegraph.map.frame.id
        )
        .is_err()
    );
    Ok(())
}
