use auki_scenegraph::{MapDefinition, QrAnchor, RigidTransform, Scenegraph};
use spatial_home::local_mapper::{compose, inverse};
use spatial_home::mobile_calibration::{Capture, align, detect};

fn capture() -> Capture {
    let frame = "local-scene:test-room:portal-ABCDEFGHIJK";
    let mut scene = Scenegraph::new(MapDefinition {
        map_id: "test".into(),
        name: None,
        domain_reference: None,
        frame: spatial_home::conventions::frame(frame).unwrap(),
    })
    .unwrap();
    scene.anchors.insert(
        "ABCDEFGHIJK".into(),
        QrAnchor {
            anchor_id: "ABCDEFGHIJK".into(),
            payload: "https://r8.hr/ABCDEFGHIJK".into(),
            side_length_m: 0.2,
            pose_in_map: RigidTransform::identity("portal-test", frame),
        },
    );
    let session = uuid::Uuid::new_v4().to_string();
    Capture {
        timestamp_ns: 100,
        width: 640,
        height: 480,
        fx: 500.,
        fy: 500.,
        cx: 320.,
        cy: 240.,
        camera_to_session: RigidTransform::identity(
            format!("phone-camera:{session}"),
            format!("phone-ar:{session}"),
        ),
        session,
        scenegraph: scene,
    }
}
fn pose(p: &RigidTransform) -> auki_odometry::Pose {
    let [w, x, y, z] = p.rotation_wxyz;
    auki_odometry::Pose {
        translation: p.translation,
        rotation_xyzw: [x, y, z, w],
    }
}
fn corners() -> [[f64; 2]; 4] {
    [[270., 190.], [370., 190.], [370., 290.], [270., 290.]]
}
#[test]
fn metric_size_and_camera_axes_come_from_reference_and_capture() {
    let mut c = capture();
    for (size, distance) in [(0.2, 1.), (0.1, 0.5), (0.3, 1.5)] {
        c.scenegraph
            .anchors
            .get_mut("ABCDEFGHIJK")
            .unwrap()
            .side_length_m = size;
        let a = align(&c, "ABCDEFGHIJK", corners()).unwrap();
        assert!((a.portal_to_session.translation[2] + distance).abs() < 1e-5);
        assert!((a.session_to_home.translation[2] - distance).abs() < 1e-5);
        assert!(a.portal_to_session.rotation_wxyz[0].abs() > 0.99999);
    }
}
#[test]
fn floor_portal_and_rotated_translated_ar_session_compose_once() {
    let mut c = capture();
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let anchor = c.scenegraph.anchors.get_mut("ABCDEFGHIJK").unwrap();
    anchor.pose_in_map.translation = [3., 0., -2.];
    anchor.pose_in_map.rotation_wxyz = [h, -h, 0., 0.];
    c.camera_to_session.translation = [5., 2., 1.];
    c.camera_to_session.rotation_wxyz = [h, 0., h, 0.];
    let a = align(&c, "ABCDEFGHIJK", corners()).unwrap();
    let observed = compose(&pose(&a.session_to_home), &pose(&a.portal_to_session)).unwrap();
    let expected = pose(&c.scenegraph.anchors["ABCDEFGHIJK"].pose_in_map);
    let delta = compose(&inverse(&expected).unwrap(), &observed).unwrap();
    assert!(delta.translation.iter().all(|v| v.abs() < 1e-6));
    assert!(delta.rotation_xyzw[3].abs() > 0.999999);
    let camera_home = compose(&pose(&a.session_to_home), &pose(&c.camera_to_session)).unwrap();
    for (got, want) in camera_home.translation.into_iter().zip([3., 1., -2.]) {
        assert!((got - want).abs() < 1e-5);
    }
}
#[test]
fn rejects_unknown_mirrored_degenerate_and_wrong_frame_inputs() {
    let mut c = capture();
    assert!(align(&c, "UNKNOWN", corners()).is_err());
    assert!(align(&c, "ABCDEFGHIJK", [[0., 0.]; 4]).is_err());
    let mut mirrored = corners();
    mirrored.reverse();
    assert!(align(&c, "ABCDEFGHIJK", mirrored).is_err());
    c.camera_to_session.to_frame_id = "wrong-session".into();
    assert!(align(&c, "ABCDEFGHIJK", corners()).is_err());
}
#[test]
fn sdk_decodes_real_qr_pixels_without_hardcoded_marker_ids() {
    let c = capture();
    let qr = qrcode::QrCode::new("https://r8.hr/ABCDEFGHIJK").unwrap();
    let mut image = vec![255u8; 640 * 480];
    for y in 0..qr.width() {
        for x in 0..qr.width() {
            if qr[(x, y)] == qrcode::Color::Dark {
                for dy in 0..6 {
                    for dx in 0..6 {
                        image[(100 + y * 6 + dy) * 640 + 200 + x * 6 + dx] = 0;
                    }
                }
            }
        }
    }
    let result = detect(&image, &c).unwrap();
    eprintln!(
        "QR fixture diagnostics: {}",
        serde_json::to_string(&result.diagnostics).unwrap()
    );
    assert_eq!(result.diagnostics.matched, 1);
    assert!(result.diagnostics.decoded >= 1);
    assert!(result.diagnostics.pose_rejection.is_none());
    assert!(
        result.diagnostics.total_us >= result.diagnostics.decode_us + result.diagnostics.pose_us
    );
    assert_eq!(result.alignment.unwrap().portal_id, "ABCDEFGHIJK");
    let mut empty=capture();empty.scenegraph.anchors.clear();
    let first=detect(&image,&empty).unwrap();
    assert!(first.alignment.is_none(),"Client must wait for Home's origin decision");
    let offer=first.origin_observation.expect("unlocalized observation offered");
    assert_eq!(offer.code,"ABCDEFGHIJK");
    assert!(offer.expected_scene.scenegraph.anchors.is_empty());
    let home=spatial_home::scene_origin::establish(&offer.expected_scene,&offer).unwrap();
    empty.scenegraph=home.scenegraph;
    assert!(detect(&image,&empty).unwrap().alignment.is_some(),"Phone localizes only after Home accepts a Portal");
    let mut unknown = capture();
    let mut other = unknown.scenegraph.anchors.remove("ABCDEFGHIJK").unwrap();
    other.anchor_id = "ZYXWVUTSRQP".into();
    other.payload = "https://r8.hr/ZYXWVUTSRQP".into();
    unknown
        .scenegraph
        .anchors
        .insert(other.anchor_id.clone(), other);
    let other = detect(&image, &unknown).unwrap();
    assert!(other.alignment.is_none());
    assert!(other.diagnostics.decoded >= 1);
    assert_eq!(other.diagnostics.matched, 0);
    let blank = detect(&vec![255; 640 * 480], &c).unwrap();
    assert!(blank.alignment.is_none());
    assert_eq!(blank.diagnostics.decoded, 0);
    // Reusing the detector cannot resurrect alignment from a previous reference
    // or blank capture. The next valid capture must still work immediately.
    assert!(detect(&image, &c).unwrap().alignment.is_some());
    let mut distant = capture();
    distant.fx = 50_000.;
    distant.fy = 50_000.;
    let rejected = detect(&image, &distant).unwrap();
    assert!(rejected.alignment.is_none());
    assert_eq!(rejected.diagnostics.matched, 1);
    assert_eq!(
        rejected.diagnostics.pose_rejection.as_deref(),
        Some("Back-facing or distant Portal")
    );
    assert!(detect(&image[..100], &c).is_err());
}
