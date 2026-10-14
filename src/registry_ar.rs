//! One capture-time AR consumer for any publisher of the registry profiles.
//! Authority: a discovered, authenticated Domain peer may assert transforms out
//! of its own frames. No peer names, device types, or mapper metadata are used.
use crate::compact_pose::FrameRef;
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::ComponentProtocolClient;
use auki_components::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const CAMERA_METADATA: &str = "spatial-harness.camera-contract/v1";

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
            if c.datatype == VideoFrame::DATATYPE && crate::camera_frame::supported(&c.schema)),
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
    let targets = homes
        .snapshots
        .values()
        .map(|s| {
            crate::conventions::registry(home_peer, &s.scenegraph.map.frame.id)
                .map(|f| FrameRef::from(&f))
        })
        .collect::<Result<Vec<_>>>()?;
    let (target, pose) = crate::transform_chain::resolve(client, sensor, row,
        &camera.frame, &camera.clock, &camera.session_id, image.timestamp_ns, &targets, None).await?;
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
        crate::ar_preview::project_annotations(&view, &optical, &calibration)?
    } else {
        Value::Null
    };
    Ok(
        json!({"sequence":image.sequence.to_string(),"capture_timestamp_ns":image.timestamp_ns.to_string(),
        "clock":output.clock,"spatial_frame":output.spatial_frame,"calibration":calibration,
        "picking":{"frame":target.id,"camera_to_map":optical,"calibration":calibration},
        "viewpoint":crate::view_camera::frame_view(&pose,&declared.definition)?,"source_frame":camera.frame,
        "width":image.payload.width,"height":image.payload.height,"encoding":image.payload.encoding,
        "ar":ar}),
    )
}
