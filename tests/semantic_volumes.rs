use spatial_home::{mobile_notes::{apply_change, ChangeMemo, ChangeNote, NoteAction},
    notes::Notes, semantic_volumes::SemanticVolume};

fn volume() -> SemanticVolume {
    SemanticVolume { id: uuid::Uuid::new_v4().to_string(), name: "Kitchen".into(),
        semantic_type: "room".into(), position: [1., 0., 2.],
        dimensions: [2., 2.5, 3.], yaw: 0.4, description:None }
}
#[test]
fn volume_round_trip_update_delete_and_frame_validation() -> anyhow::Result<()> {
    let v = volume();
    let mut request = ChangeNote { request_id: uuid::Uuid::new_v4().to_string(),
        frame: "scene-a".into(), action: NoteAction::PutVolume { volume: v.clone() } };
    let original = Notes::empty("scene-a");
    let (saved, _) = apply_change(&original, &request, "scene-a")?;
    assert!(saved.items.is_empty());
    assert_eq!(saved.volumes, vec![v.clone()]);
    let restored: Notes = serde_json::from_slice(&serde_json::to_vec(&saved)?)?;
    restored.validate("scene-a")?;
    assert_eq!(restored, saved);
    assert!(apply_change(&saved, &request, "scene-b").is_err());
    let mut updated = v.clone(); updated.dimensions[0] = 4.;
    request.action = NoteAction::PutVolume { volume: updated.clone() };
    let (saved, _) = apply_change(&saved, &request, "scene-a")?;
    assert_eq!(saved.volumes, vec![updated]);
    request.action = NoteAction::DeleteVolume { id: v.id };
    let (deleted, result) = apply_change(&saved, &request, "scene-a")?;
    assert!(result.deleted && deleted.volumes.is_empty());
    assert!(apply_change(&deleted, &request, "scene-a")?.0.volumes.is_empty());
    Ok(())
}
#[test]
fn reject_invalid_geometry_and_accept_old_note_files() -> anyhow::Result<()> {
    for dimension in [0., -1., 41., f64::NAN] {
        let mut v = volume(); v.dimensions[0] = dimension;
        assert!(v.validate().is_err());
    }
    let mut v = volume(); v.yaw = f64::INFINITY; assert!(v.validate().is_err());
    let mut v = volume(); v.name.clear(); assert!(v.validate().is_err());
    let old: Notes = serde_json::from_str(r#"{"schema":"spatial.text-notes/v1","frame":"scene-a","items":[]}"#)?;
    assert!(old.volumes.is_empty()); old.validate("scene-a")?;
    Ok(())
}
#[test]
fn ambiguous_retry_does_not_duplicate_or_reapply() -> anyhow::Result<()> {
    let request = ChangeNote { request_id: uuid::Uuid::new_v4().to_string(),
        frame: "scene-a".into(), action: NoteAction::PutVolume { volume: volume() } };
    let mut memo = ChangeMemo::default();
    let first = memo.run("phone", &request, || Ok(apply_change(&Notes::empty("scene-a"), &request, "scene-a")?.1))?;
    let second = memo.run("phone", &request, || panic!("retry must not execute"))?;
    assert_eq!(first.request_id, second.request_id);
    Ok(())
}
