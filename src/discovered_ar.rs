//! Demand-driven AR from the selected peer's typed Catalog, not runner configuration.
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{ComponentProtocolClient, ObservationRequest, ObservationSelection, RemoteObservations};
use auki_components::*;
use auki_odometry::{PoseContract, PoseUpdate, Tracking};
use serde_json::{Value, json};
use std::{sync::Arc, time::{Duration, Instant}};
use crate::{local_mapper::{compose, from_rigid}, network::CanonicalScene, sensor_preview::Sensor};

pub struct Context {
    scene: CanonicalScene,
    transform: crate::frame_transform::Received,
    fetched: Instant,
    catalog_revision: Value,
}

fn products(catalog: &CatalogSnapshot, owner: &str, datatype: &str) -> Vec<ProductReference> {
    catalog.products.iter().filter(|p| p.manifest.peer_id == owner &&
        catalog.components.iter().flat_map(|c| c.current_outputs.values()).any(|o|
            o.manifest.peer_id == owner && o.manifest.reference() == p.manifest.producer
            && o.manifest.payload.datatype() == datatype))
        .map(|p| ProductReference { peer_id: owner.into(), product_id: p.manifest.product_id.clone(), manifest_hash: p.manifest_hash.clone() }).collect()
}

async fn read<T: ContractType + serde::de::DeserializeOwned>(client: &ComponentProtocolClient,
    sensor: &Sensor, product: ProductReference, selection: ObservationSelection) -> Result<RemoteObservations<T>> {
    let query = ObservationRequest { product: product.clone(), selection };
    let result = if let Some(route) = &sensor.route {
        client.observations_exact::<T>(sensor.peer, route.clone(), query).await?
    } else { client.observations::<T>(sensor.peer, query).await? };
    ensure!(result.product.reference() == product && result.gap.is_none() && result.end.is_none(), "AR Product changed, ended, or has a gap");
    ensure!(result.producer.peer_id == sensor.reference.peer_id
        && result.producer.reference() == result.product.producer
        && result.observations.iter().all(|o| o.output == result.producer.reference()), "AR Product provenance mismatch");
    Ok(result)
}

/// A scene and its transform are one relationship: never accept an unrelated map,
/// peer, calibration epoch, or a transform to a similarly named foreign scene.
pub fn validate_alignment(scene: &CanonicalScene, relation: &crate::frame_transform::Received,
    home: &auki_scenegraph::MapSnapshot, home_peer: &str, clock: &ClockReference, timestamp: u64) -> Result<()> {
    scene.validate(&relation.product.peer_id)?;
    ensure!(relation.usable(scene), "Camera peer has no current valid transform into Home");
    let value = &relation.value;
    let evidence = value.evidence.as_ref().ok_or_else(|| anyhow!("Alignment evidence missing"))?;
    ensure!(value.target_frame.as_ref() == Some(&home.scenegraph.map.frame)
        && evidence.reference_peer_id == home_peer
        && home.scenegraph.anchors.get(&evidence.portal) == Some(&evidence.reference_anchor), "Camera transform targets a different Home scene or portal definition");
    ensure!(&evidence.clock == clock && timestamp >= evidence.capture_timestamp_ns, "Camera predates calibration or belongs to a different clock session");
    let robot = &scene.registries["local_mapping"]["robot_pose"];
    let latest = robot["capture_timestamp_ns"].as_u64().ok_or_else(|| anyhow!("Camera peer tracking unavailable"))?;
    ensure!(serde_json::to_value(clock)? == robot["clock"] && latest.abs_diff(timestamp) <= 3_000_000_000,
        "Camera peer tracking is stale or belongs to another session");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn render(client: &ComponentProtocolClient, sensor: &Sensor, row: &Value,
    image: &Observation<VideoFrame>, output: &OutputManifest,
    geometry: &Observation<PoseUpdate>, geometry_output: &OutputManifest,
    homes: &crate::local_scenes::LocalScenes, home_peer: &str,
    context: &mut Option<Context>, overlay: bool) -> Result<Value> {
    let calibration = &sensor.metadata.as_ref().ok_or_else(|| anyhow!("Camera calibration metadata missing"))?.value["calibration"];
    crate::ar_preview::validate_camera_sample(&sensor.reference, image, output, geometry, geometry_output, calibration)?;
    let (frame, optical) = resolve_pose(client, sensor, row, geometry, homes, home_peer, context).await?;
    let home = homes.snapshots.values().find(|s| s.scenegraph.map.frame.id == frame).ok_or_else(|| anyhow!("Home scene unavailable"))?;
    let view = homes.view(&home.scenegraph.map.map_id)?;
    ensure!(view["frame"] == frame, "Home geometry frame mismatch");
    let ar = if overlay { crate::ar_preview::project_view(&view, &optical, calibration)? } else { Value::Null };
    use base64::Engine;
    Ok(json!({"sequence":image.sequence,"capture_timestamp_ns":image.timestamp_ns,"clock":output.clock,
        "spatial_frame":output.spatial_frame,"calibration":calibration,"picking":{"frame":frame,"camera_to_map":optical,"calibration":calibration},
        "width":image.payload.width,"height":image.payload.height,"encoding":image.payload.encoding,
        "data":base64::engine::general_purpose::STANDARD.encode(&image.payload.bytes),"ar":ar}))
}

/// Resolve a typed sensor pose without requiring or subscribing to camera pixels.
pub async fn resolve_pose(client: &ComponentProtocolClient, sensor: &Sensor, row: &Value,
    geometry: &Observation<PoseUpdate>, homes: &crate::local_scenes::LocalScenes,
    home_peer: &str, context: &mut Option<Context>) -> Result<(String, auki_odometry::Pose)> {
    geometry.payload.validate()?;
    ensure!(row["catalog_status"] == "ready", "Camera Catalog unavailable");
    let catalog: CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
    let contract = &geometry.payload.contract;
    let Tracking::Valid { pose: camera_pose } = &geometry.payload.tracking else { anyhow::bail!("Sensor tracking unavailable") };

    // A camera can publish its capture-matched pose directly in a Home frame.
    let direct = homes.snapshots.values().find(|s|
        crate::conventions::registry(home_peer, &s.scenegraph.map.frame.id).ok().as_ref() == Some(&contract.to_frame));
    let (home, optical) = if let Some(home) = direct {
        (home, camera_pose.clone())
    } else {
        // Metadata sequences change with observations; invalidate only when an
        // exact Product identity changes, not on every Catalog refresh.
        let revision = json!([products(&catalog, &sensor.reference.peer_id, crate::frame_transform::SCHEMA),
            products(&catalog, &sensor.reference.peer_id, crate::network::SCENE_SCHEMA)]);
        if context.as_ref().is_none_or(|c| c.fetched.elapsed() > Duration::from_millis(500) || c.catalog_revision != revision) {
            *context = None;
            let transforms = products(&catalog, &sensor.reference.peer_id, crate::frame_transform::SCHEMA);
            ensure!(transforms.len() == 1, "Camera peer must advertise one unambiguous Home frame transform");
            let response = read::<crate::frame_transform::FrameTransform>(client, sensor, transforms[0].clone(), ObservationSelection::LatestExisting).await?;
            let observation = response.observations.last().ok_or_else(|| anyhow!("Waiting for camera peer calibration"))?;
            let relation = crate::frame_transform::Received { value: (*observation.payload).clone(), product: transforms[0].clone(), received: Instant::now() };
            relation.value.validate(&sensor.reference.peer_id)?;
            let scenes = products(&catalog, &sensor.reference.peer_id, crate::network::SCENE_SCHEMA);
            ensure!(scenes.len() == 1, "Camera peer must advertise one unambiguous local scene");
            let response = read::<CanonicalScene>(client, sensor, scenes[0].clone(), ObservationSelection::LatestExisting).await?;
            let scene = response.observations.last().ok_or_else(|| anyhow!("Waiting for camera peer scene"))?.payload.as_ref().clone();
            *context = Some(Context { scene, transform: relation, fetched: Instant::now(), catalog_revision: revision });
        }
        let c = context.as_ref().unwrap();
        let target = c.transform.value.target_frame.as_ref().ok_or_else(|| anyhow!("Camera peer is not calibrated into Home"))?;
        let home = homes.snapshots.values().find(|s| s.scenegraph.map.frame == *target)
            .ok_or_else(|| anyhow!("Camera transform does not target a local Home scene"))?;
        validate_alignment(&c.scene, &c.transform, home, home_peer, &contract.clock, geometry.timestamp_ns)?;
        let local_frame = crate::conventions::registry(&sensor.reference.peer_id, &c.scene.frame)?;
        let camera_local = if contract.to_frame == local_frame {
            camera_pose.clone()
        } else {
            let odometry = catalog.products.iter().filter_map(|p| {
                let metadata = p.metadata.as_ref()?;
                let pose: PoseContract = serde_json::from_value(metadata.value.clone()).ok()?;
                (pose.from_frame == contract.to_frame && pose.clock == contract.clock && pose.session_id == contract.session_id)
                    .then_some((p, pose))
            }).collect::<Vec<_>>();
            ensure!(odometry.len() == 1, "No unambiguous capture-time pose chain from camera into peer map");
            let (p, pose_contract) = &odometry[0];
            let reference = ProductReference { peer_id: sensor.reference.peer_id.clone(), product_id: p.manifest.product_id.clone(), manifest_hash: p.manifest_hash.clone() };
            ensure!(products(&catalog, &sensor.reference.peer_id, PoseUpdate::DATATYPE).contains(&reference), "Odometry Product is not a current typed pose Product");
            let response = read::<PoseUpdate>(client, sensor, reference, ObservationSelection::TimeRange {
                request: TimeRangeRequest { clock: contract.clock.clone(), start_ns: geometry.timestamp_ns.saturating_sub(200_000_000), end_ns: geometry.timestamp_ns.saturating_add(200_000_000) }
            }).await?;
            let retained = RetainedProduct::imported_buffer(response.product, response.product_manifest_hash, response.producer,
                BufferLimits { max_entries: Some(256), max_bytes: Some(1024*1024), target_duration: None }, |_: &PoseUpdate| 1024)?;
            for o in response.observations {
                retained.buffer().append_shared(Arc::new(Envelope::new(o.sequence, o.timestamp_ns, o)))?;
            }
            let base = auki_odometry::pose_at(&retained, &auki_odometry::PoseQuery {
                contract: pose_contract.clone(), timestamp_ns: geometry.timestamp_ns, max_gap_ns: 200_000_000 })?;
            let origin: crate::local_mapper::Origin = serde_json::from_value(c.scene.registries["local_mapping"]["origin"].clone())?;
            ensure!(origin.capture_clock == contract.clock && geometry.timestamp_ns >= origin.capture_timestamp_ns,
                "Camera and peer map origin belong to different sessions");
            // The scene publishes the exact odometry frame it maps, not just an axis convention.
            let frames: Vec<auki_registry::FrameRegistryEntry> = serde_json::from_value(c.scene.registries["frames"].clone())?;
            ensure!(frames.contains(&pose_contract.to_frame), "Odometry frame does not belong to peer map");
            ensure!(c.scene.registries["local_mapping"]["map_correction"]["capture_timestamp_ns"].as_u64()
                .is_none_or(|t| t <= geometry.timestamp_ns), "Camera predates the current map correction");
            compose(&origin.odometry_to_local, &compose(&base.pose, camera_pose)?)?
        };
        (home, compose(&from_rigid(c.transform.value.transform.as_ref().unwrap()), &camera_local)?)
    };
    Ok((home.scenegraph.map.frame.id.clone(), optical))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_home_view_projects_without_robot_geometry_or_mapper() -> Result<()> {
        let calibration = json!({"K":[100.,0.,50.,0.,100.,40.,0.,0.,1.],"D":[],"distortion_model":"none","width":100,"height":80});
        let view = json!({"frame":"home-conference","voxel_size":0.1,"voxels":[],
            "notes":[{"id":"note","text":"Dan","position":[0.,0.,2.]}]});
        let projected = crate::ar_preview::project_view(&view, &crate::local_mapper::identity(), &calibration)?;
        assert_eq!(projected["frame"], "home-conference");
        assert_eq!(projected["notes"][0]["pixel"], json!([50.,40.]));
        assert_eq!(projected["voxel_count"], 0);
        let mut colored = view.clone();
        colored["voxels"] = json!([{"center":[0.,0.,2.],"evidence":1.,"linear_rgb":[1.,0.,0.]}]);
        let projected = crate::ar_preview::project_view(&colored, &crate::local_mapper::identity(), &calibration)?;
        assert_eq!(projected["voxel_count"], 1);
        assert_eq!(projected["vertices"][0][3], 1.);
        assert_eq!(projected["vertices"][0][4], 0.);
        Ok(())
    }
}
