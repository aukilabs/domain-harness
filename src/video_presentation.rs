//! Presentation only. Camera bytes, calibration, poses and pixel coordinates
//! remain native. Missing orientation must not prevent viewing a video feed.
use crate::compact_pose::{Contract, FrameRef, Lifetime, Update};
use anyhow::{Result, anyhow, ensure};
use auki_components::*;
use serde_json::{Value, json};

/// World-up expressed in the image gives the nearest upright quarter turn.
/// Axes are converted with SDK geometry, not peer/model-specific rules.
pub fn upright(
    pose: &auki_odometry::Pose,
    from: &auki_registry::FrameRegistryEntry,
    to: &auki_registry::FrameRegistryEntry,
    image_turn: u16,
) -> Result<u16> {
    pose.validate()?;
    ensure!(
        [0, 90, 180, 270].contains(&image_turn),
        "Invalid image rotation"
    );
    use auki_datatypes::pose::{Quat, SpatialTransform, Vec3};
    let standard = auki_registry::FrameRegistryEntry::opengl(&to.peer_id, &to.frame_id);
    let up = auki_geometry::convert_direction_convention(
        Vec3 {
            x: 0.,
            y: 1.,
            z: 0.,
        },
        &standard,
        to,
    )?;
    let [x, y, z, w] = pose.rotation_xyzw;
    let inverse = auki_geometry::inverse_spatial_transform(&SpatialTransform {
        translation: Some(Vec3 {
            x: 0.,
            y: 0.,
            z: 0.,
        }),
        orientation: Some(Quat { x, y, z, w }),
    })?;
    let m = auki_geometry::spatial_transform_to_matrix4(&inverse)?;
    let local = Vec3 {
        x: m[0][0] * up.x + m[0][1] * up.y + m[0][2] * up.z,
        y: m[1][0] * up.x + m[1][1] * up.y + m[1][2] * up.z,
        z: m[2][0] * up.x + m[2][1] * up.y + m[2][2] * up.z,
    };
    let camera = auki_geometry::convert_direction_convention(
        local,
        from,
        &auki_registry::FrameRegistryEntry::opengl(&from.peer_id, &from.frame_id),
    )?;
    ensure!(
        camera.x.hypot(camera.y) > 0.2,
        "Camera points along up/down; upright is ambiguous"
    );
    let angle = (-camera.x).atan2(camera.y).to_degrees() - f64::from(image_turn);
    Ok(((angle / 90.).round() as i32 * 90).rem_euclid(360) as u16)
}

pub async fn resolve(
    client: &auki_component_protocol::ComponentProtocolClient,
    sensor: &crate::sensor_preview::Sensor,
    row: &Value,
    output: &OutputManifest,
    timestamp: u64,
) -> Value {
    let work = async {
        let meta = sensor.metadata.as_ref().map(|m| &m.value);
        if let Some(p) = meta.and_then(|m| m.get("presentation")) {
            ensure!(
                p["schema"] == "spatial.video-presentation/v1",
                "Unsupported presentation declaration"
            );
            let degrees = p["rotation_clockwise_deg"]
                .as_u64()
                .filter(|d| [0, 90, 180, 270].contains(d))
                .ok_or_else(|| anyhow!("Invalid declared display rotation"))?;
            return Ok(json!({"rotation_clockwise_deg":degrees,"source":"declaration"}));
        }
        let frame = output
            .spatial_frame
            .as_ref()
            .ok_or_else(|| anyhow!("Camera frame not declared"))?;
        let catalog: CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
        let candidates = catalog
            .products
            .iter()
            .filter_map(|p| {
                let m = p
                    .metadata
                    .as_ref()
                    .filter(|m| m.schema == crate::compact_pose::METADATA)?;
                let c: Contract = serde_json::from_value(m.value.clone()).ok()?;
                if c.lifetime != Lifetime::Sampled
                    || c.from_frame != FrameRef::from(&frame.definition)
                    || c.clock != output.clock
                    || p.manifest.peer_id != sensor.reference.peer_id
                {
                    return None;
                }
                let o = catalog
                    .components
                    .iter()
                    .flat_map(|c| c.current_outputs.values())
                    .find(|o| o.manifest.reference() == p.manifest.producer)?;
                c.validate_output(&o.manifest).ok()?;
                Some((p.manifest.reference(), c))
            })
            .collect::<Vec<_>>();
        ensure!(
            candidates.len() == 1,
            "No unambiguous capture-time camera pose"
        );
        let (reference, c) = &candidates[0];
        let (resolved, _) = crate::product_registry::resolved_contract(&row["registries"], c)?;
        let gap = crate::declared_depth::MAX_GAP_NS;
        let request = auki_component_protocol::ObservationRequest {
            product: reference.clone(),
            selection: auki_component_protocol::ObservationSelection::TimeRange {
                request: TimeRangeRequest {
                    clock: c.clock.clone(),
                    start_ns: timestamp.saturating_sub(gap),
                    end_ns: timestamp.saturating_add(gap),
                },
            },
        };
        let response = if let Some(route) = &sensor.route {
            client
                .observations_exact::<Update>(sensor.peer, route.clone(), request)
                .await?
        } else {
            client.observations::<Update>(sensor.peer, request).await?
        };
        ensure!(
            response.product.reference() == *reference
                && response.product.producer == response.producer.reference()
                && response.gap.is_none()
                && response.end.is_none(),
            "Pose history changed/unavailable"
        );
        c.validate_output(&response.producer)?;
        let samples = response
            .observations
            .iter()
            .map(|o| {
                ensure!(
                    o.output == response.producer.reference(),
                    "Pose provenance mismatch"
                );
                Ok((o.sequence, o.timestamp_ns, (*o.payload).clone()))
            })
            .collect::<Result<Vec<_>>>()?;
        let pose = crate::declared_depth::sampled_at(&samples, timestamp)?;
        let turn = match meta.and_then(|m| m.get("rotation_from_camera_frame_deg")) {
            Some(value) => value
                .as_u64()
                .filter(|v| [0, 90, 180, 270].contains(v))
                .ok_or_else(|| anyhow!("Invalid image/frame rotation"))?
                as u16,
            None => 0,
        };
        let rotation = upright(&pose, &resolved.from_frame, &resolved.to_frame, turn)?;
        Ok::<_, anyhow::Error>(
            json!({"rotation_clockwise_deg":rotation,"source":"capture_pose","timestamp_ns":timestamp.to_string(),"reference_frame":c.to_frame}),
        )
    };
    // A phone's pose comes over Wi-Fi; 200 ms missed often enough to lay its picture on its side.
    match tokio::time::timeout(std::time::Duration::from_millis(800), work).await {
        Ok(Ok(value)) => value,
        Ok(Err(e)) => json!({"source":"unavailable","detail":e.to_string()}),
        Err(_) => json!({"source":"unavailable","detail":"Orientation lookup timed out"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quarter_turns_and_already_rotated_images() -> Result<()> {
        let camera = auki_registry::FrameRegistryEntry::opengl("peer", "camera");
        let world = auki_registry::FrameRegistryEntry::opengl("peer", "world");
        for degrees in [0, 90, 180, 270] {
            let half = (degrees as f64).to_radians() / 2.;
            let pose = auki_odometry::Pose {
                translation: [5., 2., 9.],
                rotation_xyzw: [0., 0., half.sin(), half.cos()],
            };
            let expected = (360 - degrees) % 360;
            assert_eq!(upright(&pose, &camera, &world, 0)?, expected);
            assert_eq!(upright(&pose, &camera, &world, expected)?, 0);
        }
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert!(
            upright(
                &auki_odometry::Pose {
                    translation: [0.; 3],
                    rotation_xyzw: [h, 0., 0., h]
                },
                &camera,
                &world,
                0
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    fn optical_camera_and_z_up_world_use_declared_conventions() -> Result<()> {
        let optical = auki_registry::FrameRegistryEntry::ros_optical("peer", "camera");
        let world = auki_registry::FrameRegistryEntry::opengl("peer", "world");
        let pose = auki_odometry::Pose {
            translation: [0.; 3],
            rotation_xyzw: [1., 0., 0., 0.],
        };
        assert_eq!(upright(&pose, &optical, &world, 0)?, 0);
        let camera = auki_registry::FrameRegistryEntry::opengl("peer", "camera");
        let world = auki_registry::FrameRegistryEntry::in_convention(
            "peer",
            "world",
            auki_registry::CoordinateConvention::ZUpRightForward,
        );
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert_eq!(
            upright(
                &auki_odometry::Pose {
                    translation: [0.; 3],
                    rotation_xyzw: [h, 0., 0., h]
                },
                &camera,
                &world,
                0
            )?,
            0
        );
        Ok(())
    }
}
