use anyhow::Result;
use spatial_home::{
    model::{FRAME, Store},
    notes::{self, NewNote},
};
#[test]
fn notes_persist_and_use_home_up_not_portal_rotation() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("home.json");
    let mut store = Store::open_canonical(&path)?;
    notes::initialize(&mut store)?;
    let note = notes::add(
        &mut store,
        NewNote {
            text: "abc".into(),
            portal: "UCOF1J40N7O".into(),
            height_m: 1.,
        },
    )?;
    assert_eq!(note.position, [0., 1., 0.]);
    let notes = notes::from_registries(&store.built.registries, FRAME)?;
    let from = spatial_home::conventions::registry("home", FRAME)?;
    let to = spatial_home::conventions::registry(
        "home",
        &format!("home-live:{}:zup", uuid::Uuid::new_v4()),
    )?;
    let converted = notes.in_frame(&from, &to)?;
    assert_eq!(converted.items[0].position, [0., 0., 1.]);
    assert_eq!(converted.in_frame(&to, &from)?.items, notes.items);
    let mut reopened = Store::open_canonical(&path)?;
    notes::initialize(&mut reopened)?;
    assert_eq!(
        notes::from_registries(&reopened.built.registries, FRAME)?,
        notes
    );
    for request in [
        NewNote {
            text: "bad".into(),
            portal: "unknown".into(),
            height_m: 1.,
        },
        NewNote {
            text: "\n".into(),
            portal: "UCOF1J40N7O".into(),
            height_m: 1.,
        },
        NewNote {
            text: "bad".into(),
            portal: "UCOF1J40N7O".into(),
            height_m: f64::NAN,
        },
    ] {
        assert!(notes::add(&mut reopened, request).is_err());
    }
    assert_eq!(
        notes::from_registries(&reopened.built.registries, FRAME)?,
        notes
    );
    Ok(())
}

#[test]
fn robot_labels_follow_each_base_in_home_up_and_expire() -> Result<()> {
    use serde_json::json;
    use std::{
        collections::BTreeMap,
        time::{Duration, Instant},
    };
    let frame = spatial_home::conventions::registry(
        "home",
        &format!("home-live:{}:zup", uuid::Uuid::new_v4()),
    )?;
    let names = BTreeMap::from([
        ("a".into(), "Galbot 95".into()),
        ("b".into(), "Galbot Echo".into()),
    ]);
    let mut runners = json!({});
    for peer in ["a", "b"] {
        runners[peer] = json!({"status":"aligned","robot_pose":{"frame":frame.frame_id,"capture_timestamp_ns":10,"clock":spatial_home::model::clock_ref(&spatial_home::model::clock_entry(peer,"boot")),"pose":{"translation":if peer=="a" {[2.,3.,0.2]} else {[-2.,1.,0.4]},"rotation_xyzw":[0.,0.,1.,0.]}}});
    }
    let now = Instant::now();
    let mut labels = notes::RobotLabels::default();
    let initial = labels.update(&runners, &names, &frame, now)?;
    assert_eq!(initial.len(), 2);
    assert_eq!(initial[0].text, "Galbot 95");
    assert_eq!(initial[0].position, [2., 3., 2.]);
    assert_eq!(initial[1].position, [-2., 1., 2.2]);
    let mut envelope = notes::Notes::empty(&frame.frame_id);
    envelope.items = initial.clone();
    envelope.validate(&frame.frame_id)?;
    let displayed =
        envelope.in_frame(&frame, &spatial_home::conventions::registry("home", FRAME)?)?;
    assert_eq!(displayed.items[0].position[1], 2.);
    runners["a"]["robot_pose"]["pose"]["translation"] = json!([4., 3., 0.2]);
    runners["a"]["robot_pose"]["capture_timestamp_ns"] = json!(11);
    let moved = labels.update(&runners, &names, &frame, now + Duration::from_secs(2))?;
    assert_eq!(moved[0].id, initial[0].id);
    assert_eq!(moved[0].position, [4., 3., 2.]);
    let expired = labels.update(&runners, &names, &frame, now + Duration::from_secs(4))?;
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].text, "Galbot 95");
    runners["a"]["status"] = json!("disconnected");
    assert!(
        labels
            .update(&runners, &names, &frame, now + Duration::from_secs(4))?
            .is_empty()
    );
    Ok(())
}

#[test]
fn beam_catches_small_holes_without_inflating_corners_into_a_square() -> Result<()> {
    use spatial_home::raycast::{Voxel, beamcast, raycast};
    let near = Voxel {
        center: [0.2, 0., 2.],
        evidence: 1.,
        linear_rgb: None,
    };
    let far = Voxel {
        center: [0., 0., 4.],
        evidence: 1.,
        linear_rgb: None,
    };
    let voxels = [far.clone(), near.clone()];
    assert_eq!(
        raycast(&voxels, 0.1, [0.; 3], [0., 0., 1.], 10.)?
            .unwrap()
            .center,
        far.center
    );
    let hit = beamcast(&voxels, 0.1, [0.; 3], [0., 0., 8.], 10., 0.25)?.unwrap();
    assert_eq!(hit.center, near.center);
    assert!((hit.distance - 1.75).abs() < 1e-9);
    assert!((hit.position[0] - 0.15).abs() < 1e-9);
    assert!((hit.position[2] - 1.95).abs() < 1e-9);
    let corner = Voxel {
        center: [0.25, 0.25, 2.],
        ..near
    };
    assert!(beamcast(&[corner], 0.1, [0.; 3], [0., 0., 1.], 10., 0.25)?.is_none());
    assert!(beamcast(&[far], 0.1, [0.; 3], [0., 0., 1.], 1., 0.25)?.is_none());
    assert!(beamcast(&[], 0.1, [0.; 3], [0.; 3], 10., 0.25).is_err());
    Ok(())
}

#[test]
fn camera_pick_rays_roundtrip_measured_lens_projection() -> Result<()> {
    use spatial_home::ar_preview::{camera_model, pixel_ray, project_camera_point};
    for (model, d) in [
        ("kannala_brandt", vec![0.03, -0.005, 0.001, 0.]),
        ("plumb_bob", vec![0.1, -0.03, 0.002, -0.003, 0.001]),
        (
            "Inverse Brown Conrady",
            vec![0.1, -0.03, 0.002, -0.003, 0.001],
        ),
    ] {
        let calibration = serde_json::json!({"K":[300.,0.,320.,0.,300.,240.,0.,0.,1.],"D":d,"distortion_model":model});
        let camera = camera_model(&calibration)?;
        let pixel = project_camera_point(
            &camera,
            model,
            auki_geometry::pnp::Vector3::new(0.4, -0.3, 1.),
        )
        .unwrap();
        let ray = pixel_ray(&calibration, [pixel.x, pixel.y])?;
        assert!(
            (ray[0] - 0.4).abs() < 1e-5 && (ray[1] + 0.3).abs() < 1e-5,
            "{model}: {ray:?}"
        );
    }
    Ok(())
}

#[test]
fn deleting_one_annotation_persists_and_preserves_other_notes() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("home.json");
    let mut store = Store::open_canonical(&path)?;
    notes::initialize(&mut store)?;
    let mut ids = Vec::new();
    for text in ["remove me", "keep me"] {
        ids.push(
            notes::add(
                &mut store,
                NewNote {
                    text: text.into(),
                    portal: "UCOF1J40N7O".into(),
                    height_m: 1.,
                },
            )?
            .id,
        );
    }
    let revision = store.built.view.revision;
    notes::remove(&mut store, &ids[0])?;
    assert_eq!(store.built.view.revision, revision + 1);
    assert!(notes::remove(&mut store, &ids[0]).is_err());
    assert!(notes::remove(&mut store, "robot-name:peer").is_err());
    assert_eq!(store.built.view.revision, revision + 1);
    let mut reopened = Store::open_canonical(&path)?;
    notes::initialize(&mut reopened)?;
    let remaining = notes::from_registries(&reopened.built.registries, FRAME)?;
    assert_eq!(remaining.items.len(), 1);
    assert_eq!(remaining.items[0].id, ids[1]);
    assert_eq!(remaining.items[0].text, "keep me");
    Ok(())
}
