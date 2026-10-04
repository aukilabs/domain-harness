use anyhow::Result;
use spatial_home::{conventions, domain_portals, model::Store};
#[test]
fn home_origin_preserves_portal_rotations_and_separations() -> Result<()> {
    let raw = domain_portals::load()?;
    let home = conventions::portals(&raw, spatial_home::model::FRAME)?;
    assert_eq!(
        home.scenegraph.anchors["UCOF1J40N7O"]
            .pose_in_map
            .translation,
        [0.; 3]
    );
    for (id, old) in &raw.scenegraph.anchors {
        let new = &home.scenegraph.anchors[id];
        assert_eq!(new.pose_in_map.rotation_wxyz, old.pose_in_map.rotation_wxyz);
        for i in 0..3 {
            assert!(
                (new.pose_in_map.translation[i]
                    - (old.pose_in_map.translation[i] - conventions::HOME_ORIGIN[i]))
                    .abs()
                    < 1e-12
            );
        }
    }
    let roundtrip = conventions::portals(&home, domain_portals::FRAME)?;
    assert!(conventions::matching_portals(&roundtrip, &raw)?);
    Ok(())
}
#[test]
#[ignore = "requires SPATIAL_REBASE_CHECKPOINT; copies input without modifying it"]
fn rebase_existing_checkpoint_preserves_voxels_and_survives_restart() -> Result<()> {
    let path = std::env::var("SPATIAL_REBASE_CHECKPOINT")?;
    let old: spatial_home::runner_voxels::Checkpoint =
        serde_json::from_slice(&std::fs::read(&path)?)?;
    let tmp = tempfile::tempdir()?;
    let journal = tmp.path().join("home.json");
    std::fs::copy(path, journal.with_extension("voxels.json"))?;
    let store = Store::open_live(&journal)?;
    assert_eq!(store.built.voxels.checkpoint, old.voxels.checkpoint);
    assert_eq!(
        store.built.voxels.integrated_observations,
        old.voxels.integrated_observations
    );
    assert_eq!(
        store.built.portals.scenegraph.anchors["UCOF1J40N7O"]
            .pose_in_map
            .translation,
        [0.; 3]
    );
    let old_cells: Vec<_> = old
        .voxels
        .accumulator()?
        .viewer_snapshot(0.)?
        .chunks
        .into_iter()
        .flat_map(|c| c.voxels)
        .filter(|c| c.occupancy_evidence > 0.)
        .collect();
    assert_eq!(old_cells.len(), store.built.view.voxels.len());
    for (a, b) in old_cells.iter().zip(&store.built.view.voxels) {
        for i in 0..3 {
            assert!((a.center_m[i] - conventions::HOME_ORIGIN[i] - b.center[i]).abs() < 1e-10);
        }
        assert_eq!(a.linear_rgb, b.linear_rgb);
        assert_eq!(a.occupancy_evidence, b.evidence);
    }
    let again = Store::open_live(&journal)?;
    assert_eq!(again.built.view.voxels, store.built.view.voxels);
    assert!(
        journal
            .with_extension("before-ucof-origin.voxels.json")
            .exists()
    );
    Ok(())
}
