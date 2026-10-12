use spatial_home::{chatbot::Viewer, scene_geometry::*, semantic_volumes::Volume};
#[test]
fn copied_coordinates_snap_to_query_but_guesses_are_rejected()->anyhow::Result<()>{
 let p=[-4.292893218813452,0.,-4.292893218813452];
 assert_eq!(resolve_calculated_point([-4.292893218813453,0.,-4.292893218813453],&[p])?,p);
 assert!(resolve_calculated_point([-4.,0.,-4.],&[p]).is_err());Ok(())
}
fn volume() -> Volume {
    Volume {
        id: uuid::Uuid::new_v4().to_string(),
        title: "office".into(),
        description: "".into(),
        semantic_type: "region".into(),
        center: [2., -0.7, 3.],
        size: [10., 2., 10.],
        yaw: 0.,
    }
}
fn view() -> Viewer {
    Viewer {
        frame: "frame".into(),
        mode: "3d".into(),
        screen_right: [1., 0., 0.],
        camera_position: Some([12., 4., 13.]),
    }
}
#[test]
fn sdk_corners_and_insets_are_grounded_rotated_and_bounded() -> anyhow::Result<()> {
    let mut v = volume();
    let camera = view();
    let c = get_volume_corners(&v, &camera, "frame")?;
    assert_eq!(c["labels"]["far"], "x-:z-");
    assert_eq!(c["labels"]["right"], "x+:z-");
    let p = point_inside_volume(&v, &camera, "far", 1., "toward_center", "frame")?;
    let p: [f64; 3] = serde_json::from_value(p["position"].clone())?;
    assert!((p[0] - (-3. + 1. / 2_f64.sqrt())).abs() < 1e-12);
    assert_eq!(p[1], 0.);
    assert_eq!(
        point_inside_volume(&v, &camera, "far", 1., "from_walls", "frame")?["position"],
        serde_json::json!([-2., 0., -1.])
    );
    assert!(point_inside_volume(&v, &camera, "far", 6., "from_walls", "frame").is_err());
    v.yaw = std::f64::consts::FRAC_PI_2;
    let c = get_volume_corners(&v, &camera, "frame")?;
    let first: [f64; 3] = serde_json::from_value(c["corners"][0]["position"].clone())?;
    assert!((first[0] + 3.).abs() < 1e-10);
    assert!((first[2] - 8.).abs() < 1e-10);
    Ok(())
}
#[test]
fn viewer_offsets_project_to_ground_and_never_pass_camera() -> anyhow::Result<()> {
    let mut v = view();
    v.camera_position = Some([3., 10., 4.]);
    assert_eq!(
        offset_point(
            [0., 7., 0.],
            &v,
            "toward_viewer",
            1.,
            "viewer",
            true,
            "frame"
        )?["position"],
        serde_json::json!([0.6, 0., 0.8])
    );
    assert_eq!(
        offset_point([0.; 3], &v, "toward_viewer", 10., "viewer", true, "frame")?["position"],
        serde_json::json!([3., 0., 4.])
    );
    v.screen_right = [0., 0., -1.];
    let p = offset_point([0.; 3], &v, "right", 1., "viewer", true, "frame")?;
    let p: [f64; 3] = serde_json::from_value(p["position"].clone())?;
    assert!((p[2] + 1.).abs() < 1e-12);
    assert!(
        offset_point(
            [3., 0., 4.],
            &v,
            "toward_viewer",
            1.,
            "viewer",
            true,
            "frame"
        )
        .is_err()
    );
    Ok(())
}
#[test]
fn queries_then_move_preserve_note_identity_and_metadata() -> anyhow::Result<()> {
    use spatial_home::{
        notes::Notes,
        scene_tools::{NoteEdit, apply_note},
    };
    let frame = "local-scene:test-room:portal-HNY3JGNLRAE";
    let mut notes = Notes::empty(frame);
    apply_note(
        &mut notes,
        NoteEdit::Create {
            name: "fancy car".into(),
            description: "keep this".into(),
            position: [0.; 3],
        },
        frame,
    )?;
    let id = notes.items[0].id.clone();
    let p = point_inside_volume(&volume(), &view(), "right", 0., "toward_center", frame)?;
    apply_note(
        &mut notes,
        NoteEdit::Move {
            id: id.clone(),
            expected_position: [0.; 3],
            position: serde_json::from_value(p["position"].clone())?,
        },
        frame,
    )?;
    assert_eq!(notes.items.len(), 1);
    assert_eq!(notes.items[0].id, id);
    assert_eq!(notes.items[0].text, "fancy car");
    assert_eq!(notes.items[0].details.description, "keep this");
    Ok(())
}
