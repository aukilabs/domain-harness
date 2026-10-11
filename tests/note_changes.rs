use spatial_home::{mobile_notes::*, model::FRAME, notes::Notes};

#[test]
fn duplicate_changes_are_scoped_to_caller_and_reject_reused_ids() -> anyhow::Result<()> {
    let mut memo = ChangeMemo::default();
    let mut request = ChangeNote { request_id: uuid::Uuid::new_v4().to_string(), frame: FRAME.into(),
        action: NoteAction::Delete { id: "note".into() } };
    let result = NoteChange { request_id: request.request_id.clone(),note: None,deleted:true };
    memo.run("one",&request,|| Ok(result.clone()))?;
    memo.run("one",&request,|| panic!("duplicate applied twice"))?;
    let mut called = false;
    memo.run("two",&request,|| { called = true; Ok(result) })?;
    assert!(called);
    request.action = NoteAction::Delete { id:"different".into() };
    assert!(memo.run("one",&request,|| panic!("reused ID applied")).is_err());
    Ok(())
}

#[test]
fn surface_notes_are_bounded_isolated_and_restorable() -> anyhow::Result<()> {
    let empty = Notes::empty(FRAME);
    let id = uuid::Uuid::new_v4().to_string();
    let mut request = ChangeNote {
        request_id: uuid::Uuid::new_v4().to_string(),
        frame: FRAME.into(),
        action: NoteAction::Place {
            id: id.clone(),
            origin: [0., 1., 0.],
            position: [0., 1., -2.],
            text: "Note".into(),
        },
    };
    let (notes, created) = apply_change(&empty, &request, FRAME)?;
    assert_eq!(notes.items.len(), 1);
    assert_eq!(apply_change(&notes, &request, FRAME)?.0.items.len(), 1);
    assert!(apply_change(&notes, &request, "other").is_err());
    request.action = NoteAction::Edit {
        id: id.clone(),
        text: "".into(),
    };
    assert!(apply_change(&notes, &request, FRAME).is_err());
    assert_eq!(notes.items[0].text, "Note");
    request.action = NoteAction::Delete { id: id.clone() };
    let (deleted, result) = apply_change(&notes, &request, FRAME)?;
    assert!(result.deleted);
    assert!(deleted.items.is_empty());
    request.action = NoteAction::Restore {
        note: result.note.unwrap(),
    };
    assert_eq!(apply_change(&deleted, &request, FRAME)?.0, notes);
    let mut device = created.note.unwrap();
    device.anchor_robot = Some(spatial_home::notes::RobotAnchor {
        peer_id: "phone".into(),
        capture_timestamp_ns: 1,
        clock: spatial_home::model::clock_ref(&spatial_home::model::clock_entry(
            "phone", "session",
        )),
    });
    let protected = Notes {
        items: vec![device],
        ..empty.clone()
    };
    request.action = NoteAction::Delete { id: id.clone() };
    assert!(apply_change(&protected, &request, FRAME).is_err());
    request.action = NoteAction::Place {
        id,
        origin: [0.; 3],
        position: [0., 0., 41.],
        text: "Note".into(),
    };
    assert!(apply_change(&empty, &request, FRAME).is_err());
    Ok(())
}
