//! One capture-time AR consumer for any publisher of the registry profiles.
//! Authority: a discovered, authenticated Domain peer may assert transforms out
//! of its own frames. No peer names, device types, or mapper metadata are used.
use crate::compact_pose::{Contract, FrameRef, Lifetime, Update};
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{ComponentProtocolClient, ObservationRequest, ObservationSelection};
use auki_components::*;
use auki_odometry::Pose;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const CAMERA_METADATA: &str = "spatial-harness.camera-contract/v1";
const MAX_EDGES: usize = 64;
const MAX_DEPTH: usize = 8;
const GAP_NS: u64 = 250_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraContract {
    pub sensor: auki_registry::RegistryRef,
    pub frame: FrameRef,
    pub clock: ClockReference,
    pub session_id: String,
}

impl CameraContract {
    pub fn validate(&self, output: &OutputManifest) -> Result<()> {
        output.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            !self.session_id.is_empty()
                && self.sensor.peer_id == output.peer_id
                && self.frame.peer_id == output.peer_id
                && self.clock.peer_id == output.peer_id,
            "Camera contract authority mismatch"
        );
        ensure!(
            output.output_id == format!("camera-{}", manifest_hash(self))
                && output.clock == self.clock
                && output
                    .spatial_frame
                    .as_ref()
                    .is_some_and(|f| FrameRef::from(&f.definition) == self.frame),
            "Camera descriptor/output mismatch"
        );
        ensure!(
            matches!(&output.payload, PayloadContract::Camera(c)
            if c.datatype == VideoFrame::DATATYPE && c.schema == "auki.video-frame/v1"),
            "Unsupported camera output"
        );
        Ok(())
    }
}

fn definition<T: serde::de::DeserializeOwned>(
    registry: &Value,
    matches: impl Fn(&T) -> bool,
) -> Result<T> {
    registry["definitions"]
        .as_object()
        .into_iter()
        .flat_map(|v| v.values())
        .filter_map(|v| serde_json::from_value::<T>(v.clone()).ok())
        .find(matches)
        .ok_or_else(|| anyhow!("Waiting for exact AR Registry definition"))
}

pub fn camera(
    metadata: &CatalogProductMetadata,
    output: &OutputManifest,
    registry: &Value,
) -> Result<(CameraContract, Value)> {
    ensure!(
        metadata.schema == CAMERA_METADATA,
        "Camera requires registry-backed calibration"
    );
    let c: CameraContract = serde_json::from_value(metadata.value.clone())?;
    c.validate(output)?;
    let frame: auki_registry::FrameRegistryEntry =
        definition(registry, |f| FrameRef::from(f) == c.frame)?;
    frame.validate()?;
    let _: auki_components::clock::ClockRegistryEntry =
        definition(registry, |v| crate::model::clock_ref(v) == c.clock)?;
    let entry: auki_registry::SensorRegistryEntry =
        definition(registry, |s: &auki_registry::SensorRegistryEntry| {
            s.peer_id == c.sensor.peer_id && s.sensor_id == c.sensor.id && s.hash() == c.sensor.hash
        })?;
    let auki_registry::SensorBody::Camera(sensor) = entry.body else {
        anyhow::bail!("Registry sensor is not a camera")
    };
    sensor.validate_image_layout()?;
    ensure!(
        sensor.frame.peer_id == c.frame.peer_id
            && sensor.frame.id == c.frame.id
            && sensor.frame.hash == c.frame.hash,
        "Calibration frame mismatch"
    );
    let PayloadContract::Camera(declared) = &output.payload else {
        unreachable!()
    };
    ensure!(
        sensor.width == declared.width
            && sensor.height == declared.height
            && sensor.image_encoding == declared.encoding,
        "Calibration/image geometry mismatch"
    );
    ensure!(
        sensor.intrinsics_model == "pinhole",
        "Unsupported intrinsics model"
    );
    let intrinsics = sensor
        .calibration
        .ok_or_else(|| anyhow!("Registry camera has no numeric calibration"))?;
    let calibration = json!({"width":sensor.width,"height":sensor.height,
        "K":[intrinsics.fx,0.,intrinsics.cx,0.,intrinsics.fy,intrinsics.cy,0.,0.,1.],
        "D":intrinsics.distortion_coefficients,"distortion_model":sensor.distortion_model});
    crate::ar_preview::camera_model(&calibration)?;
    Ok((c, calibration))
}

#[derive(Clone)]
struct Edge {
    product: ProductReference,
    output: OutputManifest,
    contract: Contract,
}

fn edges(catalog: &CatalogSnapshot, camera: &CameraContract) -> Result<Vec<Edge>> {
    let mut result = Vec::new();
    for p in &catalog.products {
        let Some(m) = &p.metadata else { continue };
        if m.schema != crate::compact_pose::METADATA {
            continue;
        }
        let c: Contract = serde_json::from_value(m.value.clone())?;
        // No implicit cross-clock conversion or cross-session interpolation.
        if c.clock != camera.clock || c.session_id != camera.session_id {
            continue;
        }
        let output = catalog
            .components
            .iter()
            .flat_map(|c| c.current_outputs.values())
            .find(|o| o.manifest.reference() == p.manifest.producer)
            .ok_or_else(|| anyhow!("Transform producer unavailable"))?;
        ensure!(
            p.manifest.hash() == p.manifest_hash && p.manifest.peer_id == camera.frame.peer_id,
            "Transform Product identity mismatch"
        );
        c.validate_output(&output.manifest)?;
        result.push(Edge {
            product: p.manifest.reference(),
            output: output.manifest.clone(),
            contract: c,
        });
    }
    ensure!(
        result.len() <= MAX_EDGES,
        "AR transform graph exceeds limit"
    );
    Ok(result)
}

// Deliberately directed: publishers declare the direction they vouch for. No
// shortest-path preference silently hides a competing calibration.
fn unique_path(
    edges: &[Edge],
    start: &FrameRef,
    targets: &[FrameRef],
) -> Result<(FrameRef, Vec<usize>)> {
    fn visit(
        edges: &[Edge],
        at: &FrameRef,
        targets: &[FrameRef],
        seen: &mut Vec<FrameRef>,
        path: &mut Vec<usize>,
        found: &mut Vec<(FrameRef, Vec<usize>)>,
        budget: &mut usize,
    ) {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        if found.len() > 1 {
            return;
        }
        if targets.contains(at) {
            found.push((at.clone(), path.clone()));
            return;
        }
        if path.len() == MAX_DEPTH {
            return;
        }
        for (i, e) in edges.iter().enumerate() {
            if e.contract.from_frame != *at || seen.contains(&e.contract.to_frame) {
                continue;
            }
            seen.push(e.contract.to_frame.clone());
            path.push(i);
            visit(
                edges,
                &e.contract.to_frame,
                targets,
                seen,
                path,
                found,
                budget,
            );
            path.pop();
            seen.pop();
        }
    }
    let mut found = Vec::new();
    let mut budget = 1024;
    visit(
        edges,
        start,
        targets,
        &mut vec![start.clone()],
        &mut Vec::new(),
        &mut found,
        &mut budget,
    );
    ensure!(budget > 0, "AR transform graph search exceeds limit");
    ensure!(
        found.len() == 1,
        "No unambiguous registry transform chain into Home"
    );
    Ok(found.remove(0))
}

/// Discovery indicates contract support, not a guarantee of current tracking.
pub fn supported(p: &CatalogProductEntry, catalog: &CatalogSnapshot, registry: &Value) -> bool {
    let Some(output) = catalog
        .components
        .iter()
        .flat_map(|c| c.current_outputs.values())
        .find(|o| o.manifest.reference() == p.manifest.producer)
    else {
        return false;
    };
    p.manifest.hash() == p.manifest_hash
        && p.metadata
            .as_ref()
            .is_some_and(|m| camera(m, &output.manifest, registry).is_ok())
}

async fn at(
    client: &ComponentProtocolClient,
    sensor: &crate::sensor_preview::Sensor,
    edge: &Edge,
    timestamp: u64,
) -> Result<Pose> {
    let c = &edge.contract;
    let (start_ns, end_ns) = match c.lifetime {
        Lifetime::Held => (0, timestamp),
        Lifetime::Sampled => (
            timestamp.saturating_sub(GAP_NS),
            timestamp.saturating_add(GAP_NS),
        ),
    };
    let request = ObservationRequest {
        product: edge.product.clone(),
        selection: ObservationSelection::TimeRange {
            request: TimeRangeRequest {
                clock: c.clock.clone(),
                start_ns,
                end_ns,
            },
        },
    };
    let r = if let Some(route) = &sensor.route {
        client
            .observations_exact::<Update>(sensor.peer, route.clone(), request)
            .await?
    } else {
        client.observations::<Update>(sensor.peer, request).await?
    };
    ensure!(
        r.product.reference() == edge.product
            && r.producer.reference() == edge.output.reference()
            && r.product.producer == r.producer.reference()
            && r.gap.is_none()
            && r.end.is_none(),
        "AR transform Product changed, ended or has a gap"
    );
    c.validate_output(&r.producer)?;
    let mut samples = Vec::new();
    for o in r.observations {
        ensure!(
            o.output == r.producer.reference()
                && o.timestamp_ns >= start_ns
                && o.timestamp_ns <= end_ns,
            "AR observation provenance/time mismatch"
        );
        if let Some((seq, time, _)) = samples.last() {
            ensure!(
                o.sequence == seq + 1 && o.timestamp_ns > *time,
                "AR transform history gap/order mismatch"
            );
        }
        o.payload.validate()?;
        samples.push((o.sequence, o.timestamp_ns, (*o.payload).clone()));
    }
    sample_at(&samples, c.lifetime, timestamp)
}

fn sample_at(samples: &[(u64, u64, Update)], lifetime: Lifetime, timestamp: u64) -> Result<Pose> {
    match lifetime {
        Lifetime::Held => crate::frame_alignment::held_at(
            &samples
                .iter()
                .map(|(_, t, u)| (*t, u.clone()))
                .collect::<Vec<_>>(),
            timestamp,
        )
        .map(|v| v.0),
        Lifetime::Sampled => {
            // Only missing bounds are retryable. Loss and excessive gaps are not.
            if let Some(before) = samples.iter().rev().find(|s| s.1 <= timestamp) {
                before.2.validate()?;
                ensure!(!matches!(before.2.tracking, auki_odometry::Tracking::Lost),
                    "Tracking lost at capture");
            }
            ensure!(
                samples.iter().any(|s| s.1 <= timestamp)
                    && samples.iter().any(|s| s.1 >= timestamp),
                "Waiting for capture-matched camera transform"
            );
            let before = samples.iter().rev().find(|s| s.1 <= timestamp).unwrap();
            let after = samples.iter().find(|s| s.1 >= timestamp).unwrap();
            ensure!(after.1 - before.1 <= GAP_NS, "AR pose gap exceeds 250 ms");
            crate::declared_depth::sampled_at(samples, timestamp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid(x: f64) -> Update {
        Update {
            tracking: auki_odometry::Tracking::Valid {
                pose: Pose {
                    translation: [x, 0., 0.],
                    rotation_xyzw: [0., 0., 0., 1.],
                },
            },
        }
    }
    #[test]
    fn sampled_capture_never_extrapolates_or_crosses_loss_or_gaps() {
        let samples = vec![(0, 100, valid(0.)), (1, 200, valid(2.))];
        assert_eq!(
            sample_at(&samples, Lifetime::Sampled, 150)
                .unwrap()
                .translation,
            [1., 0., 0.]
        );
        assert!(sample_at(&samples, Lifetime::Sampled, 99).is_err());
        assert!(sample_at(&samples, Lifetime::Sampled, 201).is_err());
        let mut lost = samples.clone();
        lost[1].2 = Update {
            tracking: auki_odometry::Tracking::Lost,
        };
        assert!(
            sample_at(&lost, Lifetime::Sampled, 150)
                .unwrap_err()
                .to_string()
                .contains("lost")
        );
        assert!(sample_at(&lost, Lifetime::Sampled, 201).unwrap_err().to_string().contains("lost"));
        let mut gap = samples.clone();
        gap[1].0 = 3;
        assert!(sample_at(&gap, Lifetime::Sampled, 150).is_err());
        let mut slow = samples;
        slow[1].1 = GAP_NS + 101;
        assert!(sample_at(&slow, Lifetime::Sampled, 150).is_err());
    }
    #[test]
    fn held_state_uses_capture_time_and_never_searches_past_revocation() {
        let samples = vec![
            (0, 10, valid(1.)),
            (
                1,
                20,
                Update {
                    tracking: auki_odometry::Tracking::Lost,
                },
            ),
            (2, 30, valid(2.)),
        ];
        assert_eq!(
            sample_at(&samples, Lifetime::Held, 15).unwrap().translation[0],
            1.
        );
        assert!(sample_at(&samples, Lifetime::Held, 25).is_err());
        assert_eq!(
            sample_at(&samples, Lifetime::Held, u64::MAX)
                .unwrap()
                .translation[0],
            2.
        );
        assert!(sample_at(&samples, Lifetime::Held, 9).is_err());
    }
}

pub async fn render(
    client: &ComponentProtocolClient,
    sensor: &crate::sensor_preview::Sensor,
    row: &Value,
    image: &Observation<VideoFrame>,
    output: &OutputManifest,
    homes: &crate::local_scenes::LocalScenes,
    home_peer: &str,
    overlay: bool,
) -> Result<Value> {
    ensure!(
        row["catalog_status"] == "ready",
        "Camera Catalog unavailable"
    );
    let catalog: CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
    let product = catalog
        .products
        .iter()
        .find(|p| p.manifest.reference() == sensor.reference)
        .ok_or_else(|| anyhow!("Camera Product changed"))?;
    ensure!(
        product.manifest.hash() == product.manifest_hash
            && product.manifest.producer == output.reference()
            && image.output == output.reference(),
        "Camera observation provenance mismatch"
    );
    let (camera, calibration) = camera(
        product
            .metadata
            .as_ref()
            .ok_or_else(|| anyhow!("Camera descriptor missing"))?,
        output,
        &row["registries"],
    )?;
    ensure!(
        calibration["width"] == image.payload.width
            && calibration["height"] == image.payload.height,
        "Camera image dimensions changed"
    );
    if let PayloadContract::Camera(c) = &output.payload {
        ensure!(
            image.payload.encoding == c.encoding,
            "Camera image encoding changed"
        );
    }
    let edges = edges(&catalog, &camera)?;
    let targets = homes
        .snapshots
        .values()
        .map(|s| {
            crate::conventions::registry(home_peer, &s.scenegraph.map.frame.id)
                .map(|f| FrameRef::from(&f))
        })
        .collect::<Result<Vec<_>>>()?;
    let (target, path) = unique_path(&edges, &camera.frame, &targets)?;
    let mut pose = crate::local_mapper::identity();
    for index in path {
        let edge = &edges[index];
        crate::product_registry::resolved_contract(&row["registries"], &edge.contract)?;
        pose = crate::local_mapper::compose(
            &at(client, sensor, edge, image.timestamp_ns).await?,
            &pose,
        )?;
    }
    let declared = output
        .spatial_frame
        .as_ref()
        .ok_or_else(|| anyhow!("Camera frame missing"))?;
    let optical = crate::discovered_ar::optical_pose(&pose, &declared.definition)?;
    let home = homes
        .snapshots
        .values()
        .find(|s| s.scenegraph.map.frame.id == target.id)
        .ok_or_else(|| anyhow!("Home scene unavailable"))?;
    let view = homes.view(&home.scenegraph.map.map_id)?;
    let ar = if overlay {
        crate::ar_preview::project_view(&view, &optical, &calibration)?
    } else {
        Value::Null
    };
    use base64::Engine;
    Ok(
        json!({"sequence":image.sequence.to_string(),"capture_timestamp_ns":image.timestamp_ns.to_string(),
        "clock":output.clock,"spatial_frame":output.spatial_frame,"calibration":calibration,
        "picking":{"frame":target.id,"camera_to_map":optical,"calibration":calibration},
        "viewpoint":crate::view_camera::frame_view(&pose,&declared.definition)?,"source_frame":camera.frame,
        "width":image.payload.width,"height":image.payload.height,"encoding":image.payload.encoding,
        "data":base64::engine::general_purpose::STANDARD.encode(&image.payload.bytes),"ar":ar}),
    )
}
