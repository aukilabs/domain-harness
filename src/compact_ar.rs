//! PiP consumes only registry-backed compact pose Products.
use anyhow::{Result, anyhow, ensure};
use auki_components::*;
use crate::compact_pose::{Contract, FrameRef, Lifetime, Update};
use serde_json::{Value, json};

pub fn candidate(catalog: &CatalogSnapshot, camera: &OutputManifest) -> Result<(ProductReference, Contract)> {
    let frame = camera.spatial_frame.as_ref().ok_or_else(|| anyhow!("Camera frame missing"))?;
    let matches = catalog.products.iter().filter_map(|p| {
        let m = p.metadata.as_ref().filter(|m| m.schema == crate::compact_pose::METADATA)?;
        let c: Contract = serde_json::from_value(m.value.clone()).ok()?;
        if c.lifetime != Lifetime::Sampled || c.from_frame != FrameRef::from(&frame.definition)
            || c.clock != camera.clock || p.manifest.peer_id != camera.peer_id { return None; }
        let o = catalog.components.iter().flat_map(|c| c.current_outputs.values())
            .find(|o| o.manifest.reference() == p.manifest.producer)?;
        c.validate_output(&o.manifest).ok()?;
        Some((p.manifest.reference(), c))
    }).collect::<Vec<_>>();
    ensure!(matches.len() == 1, "No unambiguous compact camera pose Product");
    Ok(matches[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registered_opengl_camera_projects_in_front_not_behind() -> Result<()> {
        let frame = auki_registry::FrameRegistryEntry::opengl("peer", "camera");
        let pose = auki_odometry::Pose {translation:[0.,1.,0.], rotation_xyzw:[0.,0.,0.,1.]};
        let optical = crate::conventions::pose_source(&pose, &frame,
            &auki_registry::FrameRegistryEntry::ros_optical("peer", "camera"))?;
        let view = json!({"frame":"home","voxel_size":0.1,"voxels":[],"notes":[
            {"id":"front","text":"front","position":[0.,1.,-2.]},
            {"id":"behind","text":"behind","position":[0.,1.,2.]}]});
        let calibration = json!({"K":[100.,0.,50.,0.,100.,40.,0.,0.,1.],"D":[],"distortion_model":"none","width":100,"height":80});
        let projected = crate::ar_preview::project_view(&view,&optical,&calibration)?;
        assert_eq!(projected["notes"].as_array().unwrap().len(),1);
        assert_eq!(projected["notes"][0]["pixel"],json!([50.,40.]));
        Ok(())
    }
}

pub async fn render(client: &auki_component_protocol::ComponentProtocolClient,
    sensor: &crate::sensor_preview::Sensor, row: &Value,
    image: &Observation<VideoFrame>, output: &OutputManifest,
    scenes: &crate::local_scenes::LocalScenes, home: &str, overlay: bool) -> Result<Value> {
    use auki_component_protocol::{ObservationRequest, ObservationSelection};
    let catalog: CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
    let (reference, contract) = candidate(&catalog, output)?;
    let (resolved, _) = crate::product_registry::resolved_contract(&row["registries"], &contract)?;
    ensure!(image.output == output.reference() && output.peer_id == sensor.reference.peer_id,
        "Camera capture provenance mismatch");
    let metadata = &sensor.metadata.as_ref().ok_or_else(|| anyhow!("Camera calibration missing"))?.value;
    let calibration = &metadata["calibration"];
    ensure!(calibration["width"].as_u64() == Some(image.payload.width as u64)
        && calibration["height"].as_u64() == Some(image.payload.height as u64), "Calibration dimensions differ from image");
    // Rotated/preprocessed pixels require an explicit calibrated projection;
    // never silently apply native intrinsics to an unknown image transform.
    ensure!(metadata.get("rotation_from_camera_frame_deg").is_none_or(|v| v.as_u64() == Some(0)),
        "AR requires native camera pixels; nonzero image preprocessing is unsupported");
    crate::ar_preview::camera_model(calibration)?;
    let t = image.timestamp_ns;
    let gap = crate::declared_depth::MAX_GAP_NS;
    let request = ObservationRequest { product: reference.clone(), selection: ObservationSelection::TimeRange {
        request: TimeRangeRequest { clock: contract.clock.clone(), start_ns: t.saturating_sub(gap), end_ns: t.saturating_add(gap) } } };
    let response = if let Some(route) = &sensor.route {
        client.observations_exact::<Update>(sensor.peer, route.clone(), request).await?
    } else { client.observations::<Update>(sensor.peer, request).await? };
    ensure!(response.product.reference() == reference && response.product.producer == response.producer.reference()
        && response.gap.is_none() && response.end.is_none(), "Camera pose history unavailable or changed");
    contract.validate_output(&response.producer)?;
    let samples = response.observations.iter().map(|o| {
        ensure!(o.output == response.producer.reference(), "Camera pose provenance mismatch");
        Ok((o.sequence, o.timestamp_ns, (*o.payload).clone()))
    }).collect::<Result<Vec<_>>>()?;
    let native = crate::declared_depth::sampled_at(&samples, t)?;
    let mut matches = Vec::new();
    for (id, snapshot) in &scenes.snapshots {
        let frame = &snapshot.scenegraph.map.frame.id;
        let target = crate::conventions::registry(home, frame)?;
        let pose = if contract.to_frame == FrameRef::from(&target) { native.clone() } else {
            if !catalog.products.iter().any(|p| p.metadata.as_ref().is_some_and(|m|
                m.schema == crate::compact_pose::METADATA && serde_json::from_value::<Contract>(m.value.clone())
                    .is_ok_and(|c| crate::frame_alignment::validate_chain(&c, &contract, &sensor.reference.peer_id, home, frame).is_ok()))) { continue; }
            let (held, _) = crate::frame_alignment::resolve(client, sensor, row, &contract, home, &json!({"frame":frame}), t).await?;
            crate::local_mapper::compose(&held, &native)?
        };
        matches.push((id, frame, pose));
    }
    ensure!(matches.len() == 1, "No unambiguous declared camera transform into Home");
    let (id, frame, pose) = &matches[0];
    let optical = crate::conventions::pose_source(pose, &resolved.from_frame,
        &auki_registry::FrameRegistryEntry::ros_optical(&resolved.from_frame.peer_id, &resolved.from_frame.frame_id))?;
    let mut view = scenes.view(id)?;
    ensure!(view["frame"] == **frame, "Home frame changed during projection");
    // PiP AR intentionally renders annotations, not map voxels.
    view["voxels"] = json!([]);
    let ar = if overlay { crate::ar_preview::project_view(&view, &optical, calibration)? } else { Value::Null };
    use base64::Engine;
    Ok(json!({"sequence":image.sequence,"capture_timestamp_ns":t,"clock":output.clock,
        "spatial_frame":output.spatial_frame,"calibration":calibration,
        "picking":{"frame":frame,"camera_to_map":optical,"calibration":calibration},
        "width":image.payload.width,"height":image.payload.height,"encoding":image.payload.encoding,
        "data":base64::engine::general_purpose::STANDARD.encode(&image.payload.bytes),"ar":ar}))
}
