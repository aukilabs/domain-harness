//! Keep measured sRGB samples; the SDK decodes linear-light color and accumulates evidence.
use anyhow::{Result, ensure};
use auki_datatypes::{
    map::{ColorEvidenceDelta, MapUpdate},
    pose::{SpatialTransform, Vec3},
};
use auki_mappers::{Voxelizer, decode_xyz_rgb};
use auki_registry::{
    MapBody, PointField, PointFieldDataType as D, Rangefinder, RegistryRef, VoxelColorModel,
};
pub fn enable(d: &mut auki_voxel_map::VoxelMapDefinition) {
    let MapBody::Voxel(grid) = &mut d.registry.body;
    grid.color_model = Some(VoxelColorModel::AdditiveLinearRgbEvidence);
}
pub fn add(
    update: &mut MapUpdate,
    points: &[[f64; 3]],
    colors: &[Option<[u8; 3]>],
    pose: &SpatialTransform,
    size: f64,
) -> Result<()> {
    ensure!(
        colors.is_empty() || colors.len() == points.len(),
        "point/color count mismatch"
    );
    let layout = Rangefinder {
        r#type: "point_cloud".into(),
        point_step: 15,
        is_bigendian: false,
        frame_rate_hz: 1,
        frame: RegistryRef {
            peer_id: "color-decoder".into(),
            id: "sample".into(),
            hash: "local".into(),
        },
        fields: [
            ("x", 0, D::Float32),
            ("y", 4, D::Float32),
            ("z", 8, D::Float32),
            ("r", 12, D::Uint8),
            ("g", 13, D::Uint8),
            ("b", 14, D::Uint8),
        ]
        .into_iter()
        .map(|(name, offset, datatype)| PointField {
            name: name.into(),
            offset,
            datatype,
            count: 1,
        })
        .collect(),
    };
    let voxelizer = Voxelizer::new(size, 16)?;
    for (point, color) in points.iter().zip(colors) {
        if let Some(color) = color {
            let mut bytes = vec![0; 12];
            bytes.extend(color);
            let rgb = decode_xyz_rgb(&bytes, &layout)?[0].linear_rgb.unwrap();
            // SDK quantizes the exact same f64 endpoint/pose as occupancy; no camera warp or invented colors.
            let mut color_update = voxelizer.map_sensor_points(
                [Vec3 {
                    x: point[0],
                    y: point[1],
                    z: point[2],
                }],
                pose,
                1.,
            )?;
            for chunk in &mut color_update.voxel_chunks {
                for cell in &mut chunk.voxels {
                    cell.occupancy_delta = 0.;
                    cell.color = Some(ColorEvidenceDelta {
                        red_sum_delta: rgb[0],
                        green_sum_delta: rgb[1],
                        blue_sum_delta: rgb[2],
                        weight_delta: 1.,
                    });
                }
            }
            update.voxel_chunks.extend(color_update.voxel_chunks);
        }
    }
    Ok(())
}
