use spatial_home::{chatbot::Viewer, scene_geometry::*, semantic_volumes::Volume};
#[test]
fn camera_rays_use_capture_intrinsics_and_opengl_axes()->anyhow::Result<()>{
 use serde_json::json;
 let mut view=json!({"notes":[{"id":"phone-name:dan","phone_pose":{"state":"live","received_age_ms":0,"capture_timestamp_ns":123,"pose":{"from_frame_id":"phone-camera:test","to_frame_id":"frame","translation":[0,1,0],"rotation_wxyz":[1,0,0,0]},"camera_configuration_id":"config-test"}}]});
 let calibration=json!({"width":640,"height":480,"K":[320,0,320,0,320,240,0,0,1],"D":[],"distortion_model":"pinhole"});
 assert_eq!(peer_camera_ray(&view,"dan",None,"frame",None)?["direction"],json!([0.,0.,-1.]));
 assert_eq!(peer_camera_ray(&view,"dan",Some([320.,240.]),"frame",Some(&calibration))?["direction"],json!([0.,0.,-1.]));
 let ray=peer_camera_ray(&view,"dan",Some([480.,400.]),"frame",Some(&calibration))?;
 let d:[f64;3]=serde_json::from_value(ray["direction"].clone())?;
 assert!(d[0]>0.&&d[1]<0.&&d[2]<0.);
 let (hit,floor)=spatial_home::mobile_notes::intersection(&[],0.1,[0.,1.,0.],d)?.unwrap();assert!(floor);assert_eq!(hit.position[1],0.);
 assert!(peer_camera_ray(&view,"dan",Some([640.,0.]),"frame",Some(&calibration)).is_err());
 assert!(peer_camera_ray(&view,"dan",None,"other",None).is_err());
 assert!(peer_camera_ray(&view,"dan",Some([0.,0.]),"frame",None).is_err());assert!(peer_camera_ray(&view,"dan",None,"frame",None).is_ok());
 view["notes"][0]["phone_pose"]["state"]=json!("asleep");assert!(peer_camera_ray(&view,"dan",None,"frame",None).is_err());Ok(())
}
#[test]
fn explicit_elevation_overrides_peer_height()->anyhow::Result<()>{
 let p=[0.8412583126936821,0.7251265,-5.27434883850157];
 assert_eq!(placement_elevation(p,None,"place checkpoint five meters in front of dan, at Y = 0")?,[p[0],0.,p[2]]);
 assert_eq!(placement_elevation(p,Some(3.),"at Y=0")?[1],0.);
 assert_eq!(placement_elevation(p,None,"at Y = 0.5")?[1],0.5);
 assert_eq!(placement_elevation(p,None,"on the ground")?[1],0.);
 assert_eq!(placement_elevation(p,None,"in front of dan")?,p);
 assert_eq!(placement_elevation(p,Some(-1.),"below dan")?[1],-1.);
 assert!(placement_elevation(p,Some(f64::NAN),"in front of dan").is_err());Ok(())
}
#[test]
fn peer_offsets_use_pose_not_billboard_or_camera()->anyhow::Result<()>{
 use serde_json::json;
 let pose=json!({"translation":[1,0.5,3],"rotation_wxyz":[1,0,0,0],"to_frame_id":"frame"});
 let mut phone=json!({"notes":[{"id":"phone-name:dan","position":[99,99,99],"phone_pose":{"state":"awake","pose":pose}}]});
 assert_eq!(peer_relative_point(&phone,"dan","in_front",2.,"frame")?,[1.,0.5,1.]);
 assert_eq!(peer_relative_point(&phone,"dan","right",2.,"frame")?,[3.,0.5,3.]);
 let h=std::f64::consts::FRAC_1_SQRT_2;
 phone["notes"][0]["phone_pose"]["pose"]["rotation_wxyz"]=json!([h,0,h,0]);
 let p=peer_relative_point(&phone,"dan","in_front",2.,"frame")?;assert!((p[0]+1.).abs()<1e-9);assert!((p[2]-3.).abs()<1e-9);
 phone["notes"][0]["phone_pose"]["state"]=json!("asleep");assert!(peer_relative_point(&phone,"dan","in_front",2.,"frame").is_err());
 let mut body=pose.clone();body["rotation_wxyz"]=json!([h,-h,0,0]);
 let robot=json!({"notes":[{"id":"robot-name:bot"}],"local_mapping":{"runners":{"bot":{"robot_pose":{"frame":"frame","glyph_pose":body}}}}});
 assert_eq!(peer_relative_point(&robot,"bot","in_front",2.,"frame")?,[3.,0.5,3.]);
 let left=peer_relative_point(&robot,"bot","left",2.,"frame")?;assert!((left[2]-1.).abs()<1e-9);
 assert!(peer_relative_point(&robot,"bot","in_front",2.,"wrong-frame").is_err());
 assert!(peer_relative_point(&json!({}),"missing","in_front",2.,"frame").is_err());
 Ok(())
}
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
        camera_target: None,
        focus_point: None,
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
