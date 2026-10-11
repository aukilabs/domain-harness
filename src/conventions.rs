//! Coordinate changes use SDK declarations; physical origin and sensor-local axes stay fixed.
use anyhow::{Result, ensure};
use auki_datatypes::{
    map::{MapUpdate, VoxelChunkSnapshot},
    pose::{Quat, SpatialTransform, Vec3},
};
use auki_registry::{CoordinateConvention as C, FrameRegistryEntry, MapBody, RegistryRef};
use auki_scenegraph::{MapConventionConversion, MapFrame, MapSnapshot};
use auki_voxel_map::{VoxelMapDefinition, VoxelSnapshot};
use prost::Message;
use std::collections::BTreeMap;
pub const RUNNER_FRAME: &str = "domain:b2881524-954d-4979-b715-240ed8f02807:galbot-zup";
/// Translation-only Home rebase; raw imported domain coordinates stay available.
pub const HOME_ORIGIN: [f64; 3] = [-9.2878, 0., 0.2011];
fn rebase_point(mut p: [f64; 3], from: &str, to: &str, before: bool) -> [f64; 3] {
    if from != to {
        if before && from == crate::model::FRAME {
            for i in 0..3 {
                p[i] += HOME_ORIGIN[i];
            }
        }
        if !before && to == crate::model::FRAME {
            for i in 0..3 {
                p[i] -= HOME_ORIGIN[i];
            }
        }
    }
    p
}
pub fn convention(id: &str) -> Result<C> {
    match id {
        crate::model::FRAME | crate::domain_portals::FRAME => Ok(C::OpenGl),
        RUNNER_FRAME => Ok(C::ZUpRightForward),
        id if crate::local_scenes::frame_scene_id(id).is_some() => Ok(C::OpenGl),
        id if crate::local_mapper::is_local_frame(id) => Ok(if id.ends_with(":zup") {
            C::ZUpRightForward
        } else {
            C::OpenGl
        }),
        _ => anyhow::bail!("unsupported map frame"),
    }
}
pub fn frame(id: &str) -> Result<MapFrame> {
    Ok(MapFrame::in_convention(
        id,
        if id.starts_with("home-live:") {
            "Measured portal origin; independent Home map"
        } else if crate::local_mapper::is_local_frame(id) {
            "Robot boot pose; no Home alignment"
        } else if id == crate::model::FRAME {
            "UCOF1J40N7O center; original OpenGL axes preserved"
        } else {
            "Stored domain origin; convention-only conversion"
        },
        convention(id)?,
        1.,
    )?)
}
pub fn registry(owner: &str, id: &str) -> Result<FrameRegistryEntry> {
    Ok(FrameRegistryEntry::in_convention(
        owner,
        id,
        convention(id)?,
    ))
}
pub fn validate_frame(f: &MapFrame) -> Result<()> {
    f.validate()?;
    ensure!(
        f.convention == Some(convention(&f.id)?) && f.meters_per_unit == 1.,
        "map convention declaration mismatch"
    );
    Ok(())
}
pub fn portals(source: &MapSnapshot, target: &str) -> Result<MapSnapshot> {
    validate_frame(&source.scenegraph.map.frame)?;
    same_origin(&source.scenegraph.map.frame.id, target)?;
    if source.scenegraph.map.frame.id == target {
        return Ok(source.clone());
    }
    let mut input = source.clone();
    for a in input.scenegraph.anchors.values_mut() {
        a.pose_in_map.translation = rebase_point(
            a.pose_in_map.translation,
            &source.scenegraph.map.frame.id,
            target,
            true,
        );
    }
    let input = MapSnapshot::new(input.scenegraph)?;
    let mut result = MapConventionConversion::between_named_frames(
        input.scenegraph.map.frame.clone(),
        frame(target)?,
    )?
    .convert_snapshot(&input, format!("{target}-portals"))?;
    for a in result.scenegraph.anchors.values_mut() {
        if source.scenegraph.map.frame.convention == result.scenegraph.map.frame.convention {
            a.pose_in_map.rotation_wxyz = source.scenegraph.anchors[&a.anchor_id]
                .pose_in_map
                .rotation_wxyz;
        }
        a.pose_in_map.translation = rebase_point(
            a.pose_in_map.translation,
            &source.scenegraph.map.frame.id,
            target,
            false,
        );
    }
    Ok(MapSnapshot::new(result.scenegraph)?)
}
fn same_origin(a: &str, b: &str) -> Result<()> {
    if crate::local_scenes::frame_scene_id(a).is_some()
        || crate::local_scenes::frame_scene_id(b).is_some()
    {
        ensure!(
            a == b,
            "coordinate conversion cannot align independent scenegraphs"
        );
    }
    if crate::local_mapper::is_local_frame(a) || crate::local_mapper::is_local_frame(b) {
        ensure!(
            crate::local_mapper::is_local_frame(a)
                && crate::local_mapper::is_local_frame(b)
                && crate::local_mapper::display_frame(a)? == crate::local_mapper::display_frame(b)?,
            "coordinate conversion cannot align independent physical origins"
        );
    }
    Ok(())
}
pub fn point(p: [f64; 3], from: &FrameRegistryEntry, to: &FrameRegistryEntry) -> Result<[f64; 3]> {
    same_origin(&from.frame_id, &to.frame_id)?;
    let p = rebase_point(p, &from.frame_id, &to.frame_id, true);
    let p = auki_geometry::convert_point_convention(
        Vec3 {
            x: p[0],
            y: p[1],
            z: p[2],
        },
        from,
        to,
    )?;
    Ok(rebase_point(
        [p.x, p.y, p.z],
        &from.frame_id,
        &to.frame_id,
        false,
    ))
}
pub fn pose(
    p: &auki_odometry::Pose,
    from: &FrameRegistryEntry,
    to: &FrameRegistryEntry,
) -> Result<auki_odometry::Pose> {
    same_origin(&from.frame_id, &to.frame_id)?;
    p.validate()?;
    let translation = rebase_point(p.translation, &from.frame_id, &to.frame_id, true);
    let [x, y, z, w] = p.rotation_xyzw;
    let t = auki_geometry::convert_transform_target_convention(
        &SpatialTransform {
            translation: Some(Vec3 {
                x: translation[0],
                y: translation[1],
                z: translation[2],
            }),
            orientation: Some(Quat { x, y, z, w }),
        },
        from,
        to,
    )?;
    let p = t.translation.unwrap();
    let q = t.orientation.unwrap();
    Ok(auki_odometry::Pose {
        translation: rebase_point([p.x, p.y, p.z], &from.frame_id, &to.frame_id, false),
        rotation_xyzw: [q.x, q.y, q.z, q.w],
    })
}
pub fn matching_portals(a: &MapSnapshot, b: &MapSnapshot) -> Result<bool> {
    let a = portals(a, &b.scenegraph.map.frame.id)?;
    Ok(
        a.scenegraph.map.domain_reference == b.scenegraph.map.domain_reference
            && a.scenegraph.anchors.len() == b.scenegraph.anchors.len()
            && a.scenegraph.anchors.iter().all(|(id, a)| {
                b.scenegraph
                    .anchors
                    .get(id)
                    .is_some_and(|b| crate::model::same_anchor(a, b))
            }),
    )
}
/// Re-express only a pose's input axes (for example, a display glyph's body
/// convention). The destination frame and physical position are unchanged.
pub fn pose_source(
    p: &auki_odometry::Pose,
    from: &FrameRegistryEntry,
    to: &FrameRegistryEntry,
) -> Result<auki_odometry::Pose> {
    p.validate()?;
    let [x, y, z, w] = p.rotation_xyzw;
    let t = auki_geometry::convert_transform_source_convention(
        &SpatialTransform {
            translation: Some(Vec3 {
                x: p.translation[0],
                y: p.translation[1],
                z: p.translation[2],
            }),
            orientation: Some(Quat { x, y, z, w }),
        },
        from,
        to,
    )?;
    let p = t.translation.unwrap();
    let q = t.orientation.unwrap();
    Ok(auki_odometry::Pose {
        translation: [p.x, p.y, p.z],
        rotation_xyzw: [q.x, q.y, q.z, q.w],
    })
}
/// Exact lattice-preserving migration between supported metre, signed-permutation conventions.
/// Reindex all occupied AND free cells, retaining f64 occupancy/color evidence unchanged.
pub fn voxel_checkpoint(
    source: &VoxelSnapshot,
    definition: VoxelMapDefinition,
) -> Result<VoxelSnapshot> {
    source.accumulator()?;
    definition.validate()?;
    validate_frame(&source.definition.portal_map.frame)?;
    validate_frame(&definition.portal_map.frame)?;
    let size = source.definition.grid().voxel_size_m.0;
    ensure!(
        size == definition.grid().voxel_size_m.0,
        "voxel migration cannot change resolution"
    );
    let mut update = MapUpdate::decode(source.checkpoint.as_slice())?;
    let checkpoint = update.checkpoint.as_mut().unwrap();
    let mut chunks: BTreeMap<[i32; 3], Vec<auki_datatypes::map::VoxelSnapshot>> = BTreeMap::new();
    for chunk in &checkpoint.voxel_chunks {
        for v in &chunk.voxels {
            let origin = [chunk.chunk_x, chunk.chunk_y, chunk.chunk_z];
            let local = [v.x, v.y, v.z];
            let center = std::array::from_fn(|i| {
                (f64::from(origin[i]) * 16. + f64::from(local[i]) + 0.5) * size
                    + source.definition.grid_origin_m[i]
            });
            let converted = point(center, &source.definition.frame, &definition.frame)?;
            let converted: [f64; 3] =
                std::array::from_fn(|i| converted[i] - definition.grid_origin_m[i]);
            ensure!(
                converted.iter().all(|v| v.is_finite()
                    && (v / size).floor() >= f64::from(i32::MIN)
                    && (v / size).floor() <= f64::from(i32::MAX)),
                "converted voxel index out of range"
            );
            let index = converted.map(|v| (v / size).floor() as i32);
            let mut cell = v.clone();
            cell.x = index[0].rem_euclid(16) as u32;
            cell.y = index[1].rem_euclid(16) as u32;
            cell.z = index[2].rem_euclid(16) as u32;
            chunks
                .entry(index.map(|v| v.div_euclid(16)))
                .or_default()
                .push(cell);
        }
    }
    checkpoint.voxel_chunks = chunks
        .into_iter()
        .map(|(c, voxels)| VoxelChunkSnapshot {
            chunk_x: c[0],
            chunk_y: c[1],
            chunk_z: c[2],
            voxels,
        })
        .collect();
    let mut result = source.clone();
    result.checkpoint = update.encode_to_vec();
    if let Some(o) = &mut result.latest_observation {
        let t = &mut o.pose.sensor_to_map;
        let [w, x, y, z] = t.rotation_wxyz;
        let p = pose(
            &auki_odometry::Pose {
                translation: t.translation,
                rotation_xyzw: [x, y, z, w],
            },
            &source.definition.frame,
            &definition.frame,
        )?;
        let [x, y, z, w] = p.rotation_xyzw;
        t.rotation_wxyz = [w, x, y, z];
        t.translation = p.translation;
        t.to_frame_id = definition.frame.frame_id.clone();
        o.pose.portal_snapshot = definition.portal_snapshot.clone();
    }
    result.definition = definition;
    result.accumulator()?;
    Ok(result)
}
pub fn set_definition_frame(
    d: &mut VoxelMapDefinition,
    owner: &str,
    portals: &MapSnapshot,
) -> Result<()> {
    let target = registry(owner, &portals.scenegraph.map.frame.id)?;
    // A change of physical origin must move the lattice origin too. Otherwise
    // independent mappers quantize the same surface onto offset cell centers.
    d.grid_origin_m = point(d.grid_origin_m, &d.frame, &target)?;
    d.portal_map = portals.scenegraph.map.clone();
    d.frame = target;
    let MapBody::Voxel(grid) = &mut d.registry.body;
    grid.frame = RegistryRef {
        peer_id: d.frame.peer_id.clone(),
        id: d.frame.frame_id.clone(),
        hash: d.frame.hash(),
    };
    d.registry.peer_id = owner.into();
    Ok(())
}
