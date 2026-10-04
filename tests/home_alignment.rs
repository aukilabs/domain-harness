use anyhow::Result;
use auki_scenegraph::{MapConventionConversion, MapSnapshot};
use spatial_home::{conventions, domain_portals, home_alignment::HomePortals};

#[test]
fn references_align_full_pose_and_follow_new_observations() -> Result<()> {
    let home = conventions::portals(&domain_portals::load()?, spatial_home::model::FRAME)?;
    let local_frame = format!("galbot-local:{}:zup", uuid::Uuid::new_v4());
    let mut observed = MapConventionConversion::between_named_frames(
        home.scenegraph.map.frame.clone(),
        conventions::frame(&local_frame)?,
    )?
    .convert_snapshot(&home, "robot-map")?
    .scenegraph;
    observed.map.domain_reference = None;
    observed.anchors.retain(|id, _| id == "UCOF1J40N7O");
    let a = observed.anchors.get_mut("UCOF1J40N7O").unwrap();
    a.pose_in_map.translation = [1.2, -0.3, 0.08];
    // Independent robot origin AND orientation, not just an axes permutation.
    a.pose_in_map.rotation_wxyz = [0.5, 0.5, 0.5, 0.5];
    let observed = MapSnapshot::new(observed)?;
    let source = HomePortals {
        portals: home.clone(),
        peer_id: "home".into(),
        revision: 7,
        anchor: "UCOF1J40N7O".into(),
    };
    let (aligned, meta) = source.align(&observed)?;
    aligned.validate()?;
    assert_eq!(aligned.scenegraph.anchors.len(), 14);
    assert_eq!(aligned.scenegraph.map, observed.scenegraph.map);
    assert_eq!(meta["anchor"], "UCOF1J40N7O");
    let blue = &aligned.scenegraph.anchors["UCOF1J40N7O"];
    let red = &aligned.scenegraph.anchors["home:UCOF1J40N7O"];
    for i in 0..3 {
        assert!((blue.pose_in_map.translation[i] - red.pose_in_map.translation[i]).abs() < 1e-10);
    }
    let dot: f64 = blue
        .pose_in_map
        .rotation_wxyz
        .iter()
        .zip(red.pose_in_map.rotation_wxyz)
        .map(|(a, b)| a * b)
        .sum();
    assert!((dot.abs() - 1.).abs() < 1e-10);
    for (id, a) in &home.scenegraph.anchors {
        let b = &aligned.scenegraph.anchors[&format!("home:{id}")];
        let d =
            |x: [f64; 3], y: [f64; 3]| x.iter().zip(y).map(|(a, b)| (a - b).powi(2)).sum::<f64>();
        assert!(
            (d(a.pose_in_map.translation, [0.; 3])
                - d(b.pose_in_map.translation, blue.pose_in_map.translation))
            .abs()
                < 1e-8
        );
        assert_eq!(a.side_length_m, b.side_length_m);
    }
    let mut updated = observed.scenegraph.clone();
    let anchor = updated.anchors.get_mut("UCOF1J40N7O").unwrap();
    anchor.pose_in_map.translation = [2., 3., 4.];
    anchor.pose_in_map.rotation_wxyz = [1., 0., 0., 0.];
    let (new, _) = source.align(&MapSnapshot::new(updated)?)?;
    assert_eq!(
        new.scenegraph.anchors["UCOF1J40N7O"]
            .pose_in_map
            .translation,
        [2., 3., 4.]
    );
    assert_ne!(
        new.scenegraph.anchors["home:FNPUME0T93U"]
            .pose_in_map
            .translation,
        aligned.scenegraph.anchors["home:FNPUME0T93U"]
            .pose_in_map
            .translation
    );
    let mut empty = observed.scenegraph.clone();
    empty.anchors.clear();
    assert!(source.align(&MapSnapshot::new(empty)?).is_err());
    Ok(())
}
