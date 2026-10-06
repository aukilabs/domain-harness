use anyhow::Result;
use spatial_home::{conventions, domain_portals, landmarks::*};

fn fixture() -> Result<Landmarks> {
    portal_map(&domain_portals::load()?, None, &Default::default())
}

#[test]
fn identity_only_schema_rejects_feature_fields() -> Result<()> {
    let expected = fixture()?;
    let envelope = serde_json::json!({"landmarks":expected});
    assert_eq!(from_registries(&envelope, &expected.frame)?, expected);
    let mut invalid = envelope.clone();
    invalid["landmarks"]["schema"] = serde_json::json!("spatial.landmarks/v1");
    assert!(from_registries(&invalid, &expected.frame).is_err());
    let mut invalid = envelope.clone();
    invalid["landmarks"]["landmarks"]["UCOF1J40N7O/top_left"]["recognition"]["feature"] =
        serde_json::json!("top_left");
    assert!(from_registries(&invalid, &expected.frame).is_err());
    for malformed in [serde_json::json!(42), serde_json::json!({})] {
        let mut invalid = envelope.clone();
        invalid["landmarks"]["landmarks"]["UCOF1J40N7O/top_left"] = malformed;
        assert!(from_registries(&invalid, &expected.frame).is_err());
    }
    Ok(())
}

#[test]
fn portals_create_four_named_points_and_a_full_pose_without_fake_detections() -> Result<()> {
    let nodes = fixture()?;
    assert_eq!(nodes.anchors.len(), 13);
    assert_eq!(nodes.by_type(PORTAL).count(), 52);
    let snapshot = domain_portals::load()?;
    for anchor in nodes.anchors.values() {
        assert!(anchor.provenance.detected_by.is_none());
        let h = snapshot.scenegraph.anchors[&anchor.id].side_length_m / 2.;
        for (name, position) in
            CORNERS
                .into_iter()
                .zip([[-h, h, 0.], [h, h, 0.], [h, -h, 0.], [-h, -h, 0.]])
        {
            let corner = &nodes.landmarks[&format!("{}/{}", anchor.id, name)];
            assert_eq!(corner.parent_anchor.as_deref(), Some(anchor.id.as_str()));
            assert_eq!(corner.position, position);
            assert_eq!(
                corner.recognition.identity,
                format!("{}/{name}", anchor.recognition.identity)
            );
            assert!(
                serde_json::to_value(&corner.recognition)?
                    .get("feature")
                    .is_none()
            );
            assert!(serde_json::to_value(corner)?.get("orientation").is_none());
        }
    }
    let decoded: Landmarks = serde_json::from_slice(&serde_json::to_vec(&nodes)?)?;
    assert_eq!(decoded, nodes);
    Ok(())
}

#[test]
fn moving_an_anchor_moves_all_children_and_convention_conversion_preserves_positions() -> Result<()>
{
    let mut nodes = fixture()?;
    let anchor = nodes.anchors.get_mut("UCOF1J40N7O").unwrap();
    anchor.pose.translation = [1., 2., 3.];
    let c = std::f64::consts::FRAC_1_SQRT_2;
    anchor.pose.rotation_wxyz = [c, 0., 0., c]; // 90 degrees around Z.
    let id = "UCOF1J40N7O/top_left";
    let local = nodes.landmarks[id].position;
    let expected = [1. - local[1], 2. + local[0], 3.];
    for (actual, expected) in nodes.position_in_map(id)?.into_iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-10);
    }
    let converted = nodes.in_frame(conventions::RUNNER_FRAME)?;
    assert_eq!(converted.landmarks[id].position, local);
    let from = conventions::registry("home", &nodes.frame)?;
    let to = conventions::registry("home", &converted.frame)?;
    for landmark in nodes.landmarks.values() {
        let expected = conventions::point(nodes.position_in_map(&landmark.id)?, &from, &to)?;
        let actual = converted.position_in_map(&landmark.id)?;
        for (a, b) in actual.into_iter().zip(expected) {
            assert!((a - b).abs() < 1e-9);
        }
    }
    Ok(())
}

#[test]
fn other_mappers_can_create_standalone_landmarks_or_anchors_without_children() -> Result<()> {
    let mut nodes = fixture()?;
    nodes.landmarks.clear();
    nodes.validate(&nodes.frame)?; // Anchors do not require children.
    let feature = Landmark {
        id: "wall-feature-42".into(),
        recognition: Recognition {
            landmark_type: IMAGE.into(),
            identity: "reference-image:42/keypoint:6".into(),
        },
        position: [1., 2., 3.],
        frame: nodes.frame.clone(),
        parent_anchor: None,
        provenance: Default::default(),
    };
    nodes.landmarks.insert(feature.id.clone(), feature);
    nodes.validate(&nodes.frame)?;
    assert_eq!(nodes.by_type(IMAGE).count(), 1);
    assert_eq!(nodes.by_type(PORTAL).count(), 0);
    assert_eq!(nodes.position_in_map("wall-feature-42")?, [1., 2., 3.]);
    let target = nodes.in_frame(conventions::RUNNER_FRAME)?;
    assert_eq!(
        target.landmarks["wall-feature-42"].frame,
        conventions::RUNNER_FRAME
    );
    let mut broken = nodes.clone();
    broken
        .landmarks
        .get_mut("wall-feature-42")
        .unwrap()
        .parent_anchor = Some("missing".into());
    assert!(broken.validate(&broken.frame).is_err());
    let mut broken = nodes.clone();
    broken
        .landmarks
        .get_mut("wall-feature-42")
        .unwrap()
        .position[0] = f64::NAN;
    assert!(broken.validate(&broken.frame).is_err());
    let mut broken = nodes.clone();
    broken.landmarks.get_mut("wall-feature-42").unwrap().frame = "other-map".into();
    assert!(broken.validate(&broken.frame).is_err());
    assert!(from_registries(&serde_json::json!({"landmarks":null}), &nodes.frame).is_err());
    assert!(
        from_registries(&serde_json::json!({}), &nodes.frame)?
            .landmarks
            .is_empty()
    );
    Ok(())
}

#[test]
fn duplicate_reference_views_keep_distinct_scene_ids_but_same_recognition() -> Result<()> {
    let mut graph = domain_portals::load()?.scenegraph;
    let mut duplicate = graph.anchors["UCOF1J40N7O"].clone();
    duplicate.anchor_id = "home:UCOF1J40N7O".into();
    graph.anchors.insert(duplicate.anchor_id.clone(), duplicate);
    let nodes = portal_map(
        &auki_scenegraph::MapSnapshot::new(graph)?,
        None,
        &Default::default(),
    )?;
    let a = &nodes.landmarks["UCOF1J40N7O/top_left"];
    let b = &nodes.landmarks["home:UCOF1J40N7O/top_left"];
    assert_ne!(a.id, b.id);
    assert_ne!(a.frame, b.frame);
    assert_eq!(a.recognition, b.recognition);
    assert_eq!(nodes.position_in_map(&a.id)?, nodes.position_in_map(&b.id)?);
    Ok(())
}
