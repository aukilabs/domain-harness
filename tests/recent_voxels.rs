use anyhow::Result;
use auki_datatypes::{
    map::{MapUpdate, VoxelSnapshot},
    pose::{Quat, SpatialTransform, Vec3},
};
use auki_mappers::Voxelizer;
use prost::Message;
use spatial_home::{
    local_mapper::{LocalMap, SensorEvent, identity},
    recent_voxels::RecentVoxels,
};
fn scan(xs: &[f64], rgb: [u8; 3]) -> Result<(MapUpdate, MapUpdate)> {
    let pose = SpatialTransform {
        translation: Some(Vec3 {
            x: 0.05,
            y: 0.05,
            z: 0.05,
        }),
        orientation: Some(Quat {
            x: 0.,
            y: 0.,
            z: 0.,
            w: 1.,
        }),
    };
    let points: Vec<_> = xs.iter().map(|x| [*x, 0., 0.]).collect();
    let make = || {
        points.iter().map(|p| Vec3 {
            x: p[0],
            y: p[1],
            z: p[2],
        })
    };
    let v = Voxelizer::new(0.1, 16)?;
    let mut rays = v.map_sensor_rays(make(), &pose, -1., 1.)?;
    spatial_home::voxel_color::add(&mut rays, &points, &vec![Some(rgb); xs.len()], &pose, 0.1)?;
    Ok((rays, v.map_sensor_points(make(), &pose, 1.)?))
}
fn cell(update: &MapUpdate, x: u32) -> &VoxelSnapshot {
    update
        .checkpoint
        .as_ref()
        .unwrap()
        .voxel_chunks
        .iter()
        .find(|c| c.chunk_x == 0 && c.chunk_y == 0 && c.chunk_z == 0)
        .unwrap()
        .voxels
        .iter()
        .find(|v| v.x == x && v.y == 0 && v.z == 0)
        .unwrap()
}
#[test]
fn long_observed_object_clears_after_two_free_observations_and_reappears() -> Result<()> {
    let mut recent = RecentVoxels::default();
    let (r, h) = scan(&[0.5], [255, 0, 0])?;
    for _ in 0..1000 {
        recent.observe(&r, &h)?;
    }
    let state = recent.observe(&r, &h)?;
    assert_eq!(cell(&state, 5).occupancy_evidence, 3.);
    // A nearer obstruction must not clear cells behind it.
    let (r, h) = scan(&[0.2], [0, 255, 0])?;
    let hidden = recent.observe(&r, &h)?;
    assert_eq!(cell(&hidden, 5).occupancy_evidence, 3.);
    // Seeing the background behind the removed object is positive free evidence.
    let (r, h) = scan(&[1.0], [0, 255, 0])?;
    let first = recent.observe(&r, &h)?;
    assert_eq!(cell(&first, 5).occupancy_evidence, 1.);
    let cleared = recent.observe(&r, &h)?;
    assert!(cell(&cleared, 5).occupancy_evidence < 0.);
    assert!(cell(&cleared, 5).color.is_none());
    for _ in 0..1000 {
        recent.observe(&r, &h)?;
    }
    let (r, h) = scan(&[0.5], [0, 0, 255])?;
    assert!(cell(&recent.observe(&r, &h)?, 5).occupancy_evidence < 0.);
    let back = recent.observe(&r, &h)?;
    assert!(cell(&back, 5).occupancy_evidence > 0.);
    let color = cell(&back, 5).color.as_ref().unwrap();
    assert_eq!(color.red_sum, 0.);
    assert_eq!(color.blue_sum / color.weight, 1.);
    // Validate the actual SDK checkpoint path and a checkpoint round trip.
    let runtime = auki_components::ComponentRuntime::new("test");
    let mut map = LocalMap::new(&runtime, "test", &uuid::Uuid::new_v4().to_string())?;
    map.accept(SensorEvent::Odometry {
        timestamp_ns: 1,
        pose: identity(),
    })?;
    let snapshot = map.voxels.unwrap();
    let mut accumulator = snapshot.accumulator()?;
    accumulator.apply(&back)?;
    let checkpoint = accumulator.checkpoint_update().encode_to_vec();
    // Populated standalone audit checkpoint has no observation provenance.
    let decoded = MapUpdate::decode(checkpoint.as_slice())?;
    assert_eq!(cell(&decoded, 5).occupancy_evidence, 1.);
    Ok(())
}
#[test]
fn scan_density_does_not_outvote_later_scans_and_hits_win_over_crossing_rays() -> Result<()> {
    let mut recent = RecentVoxels::default();
    let mut points = vec![1.0; 100];
    points.push(0.5);
    let (r, h) = scan(&points, [255, 255, 255])?;
    let s = recent.observe(&r, &h)?;
    assert_eq!(cell(&s, 5).occupancy_evidence, 1.);
    assert_eq!(cell(&s, 10).occupancy_evidence, 1.);
    let (r, h) = scan(&[1.0], [255, 255, 255])?;
    let s = recent.observe(&r, &h)?;
    assert_eq!(cell(&s, 5).occupancy_evidence, 0.);
    Ok(())
}

// Compact, valid 16^3 chunks let the test cross the production capacity without
// constructing an enormous synthetic sensor frame or changing the limit.
fn free_cells(start: usize, count: usize) -> MapUpdate {
    use auki_datatypes::map::{VoxelChunkUpdate, VoxelDelta};
    let mut chunks = std::collections::BTreeMap::<i32, Vec<VoxelDelta>>::new();
    for i in start..start + count {
        chunks
            .entry((i / 4096) as i32)
            .or_default()
            .push(VoxelDelta {
                x: (i % 16) as u32,
                y: (i / 16 % 16) as u32,
                z: (i / 256 % 16) as u32,
                occupancy_delta: -1.,
                ..Default::default()
            });
    }
    MapUpdate {
        voxel_chunks: chunks
            .into_iter()
            .map(|(chunk_x, voxels)| VoxelChunkUpdate {
                chunk_x,
                chunk_y: 10,
                chunk_z: 0,
                voxels,
            })
            .collect(),
        checkpoint: None,
    }
}
fn count(update: &MapUpdate) -> usize {
    update
        .checkpoint
        .as_ref()
        .unwrap()
        .voxel_chunks
        .iter()
        .map(|c| c.voxels.len())
        .sum()
}
fn has_free_cell(update: &MapUpdate, i: usize) -> bool {
    update
        .checkpoint
        .as_ref()
        .unwrap()
        .voxel_chunks
        .iter()
        .any(|c| {
            c.chunk_x == (i / 4096) as i32
                && c.chunk_y == 10
                && c.chunk_z == 0
                && c.voxels.iter().any(|v| {
                    v.x == (i % 16) as u32
                        && v.y == (i / 16 % 16) as u32
                        && v.z == (i / 256 % 16) as u32
                })
        })
}

#[test]
fn capacity_retires_old_free_space_and_preserves_surface_color_and_clearing_history() -> Result<()>
{
    let mut recent = RecentVoxels::default();
    let (r, h) = scan(&[0.5], [255, 0, 0])?;
    for _ in 0..3 {
        recent.observe(&r, &h)?;
    }
    let empty = MapUpdate::default();
    let cap = auki_voxel_map::MAX_VOXELS;
    // Exactly fill an existing map, then refresh one old free cell. The next
    // observation overflows it, as the real Home map did after broader sampling.
    let before = recent.observe(&free_cells(0, cap - 6), &empty)?;
    assert_eq!(count(&before), cap);
    recent.observe(&free_cells(0, 1), &empty)?;
    let after = recent.observe(&free_cells(cap, 1), &empty)?;
    assert!(count(&after) <= cap);
    assert!(has_free_cell(&after, 0));
    assert!(!has_free_cell(&after, 1));
    assert_eq!(cell(&after, 5), cell(&before, 5));
    let (free_ray, endpoint) = scan(&[1.0], [0, 255, 0])?;
    let first_free = recent.observe(&free_ray, &endpoint)?;
    assert_eq!(cell(&first_free, 5).occupancy_evidence, 1.);
    // Force several more capacity events while the surface is occluded. Its
    // mixed history and old color must survive every eviction pass unchanged.
    for batch in 1..=3 {
        let update = recent.observe(&free_cells(batch * cap, cap), &empty)?;
        assert!(count(&update) <= cap);
        assert_eq!(cell(&update, 5), cell(&first_free, 5));
    }
    let cleared = recent.observe(&free_ray, &endpoint)?;
    assert_eq!(cell(&cleared, 5).occupancy_evidence, -1.);
    assert!(cell(&cleared, 5).color.is_none());
    let pressure = recent.observe(&free_cells(4 * cap, cap), &empty)?;
    assert_eq!(cell(&pressure, 5), cell(&cleared, 5));
    // Actual SDK replacement removes evicted free cells rather than leaving
    // them in its accumulator, and the retained snapshot stays within capacity.
    let runtime = auki_components::ComponentRuntime::new("capacity-test");
    let mut map = LocalMap::new(&runtime, "capacity-test", &uuid::Uuid::new_v4().to_string())?;
    map.accept(SensorEvent::Odometry {
        timestamp_ns: 1,
        pose: identity(),
    })?;
    let mut accumulator = map.voxels.unwrap().accumulator()?;
    accumulator.apply(&before)?;
    accumulator.apply(&after)?;
    assert_eq!(accumulator.checkpoint_update(), after);
    Ok(())
}

#[test]
fn occupied_capacity_is_not_silently_erased_to_accept_more_geometry() -> Result<()> {
    let mut recent = RecentVoxels::default();
    let mut occupied = free_cells(0, auki_voxel_map::MAX_VOXELS + 1);
    for chunk in &mut occupied.voxel_chunks {
        for v in &mut chunk.voxels {
            v.occupancy_delta = 1.;
        }
    }
    let error = recent.observe(&occupied, &occupied).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("occupied voxel history exceeds map bound")
    );
    Ok(())
}
