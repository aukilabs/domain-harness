//! First real sensor input: fixed base LiDAR, capture-time odometry, SDK voxel evidence.
use crate::model::{FRAME, Store};
use anyhow::{Result, anyhow, ensure};
use auki_components::{ClockReference, Observation, OutputManifest, ProductReference};
use auki_odometry::Pose;
use auki_registry::FrameRegistryEntry;
use auki_scenegraph::{MapSnapshot, RigidTransform};
use auki_voxel_map::{
    DepthObservation, PortalVoxelMapper, SensorBinding, TimedSensorPose, VoxelSnapshot,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SensorCalibration {
    pub sensor_frame: String,
    pub sensor_to_base: Pose,
    pub calibration_source: String,
}
pub type LidarConfig = SensorCalibration;
impl SensorCalibration {
    pub fn validate(&self) -> Result<()> {
        self.sensor_to_base.validate()?;
        ensure!(
            (self.sensor_frame == "lidar_base_link"
                || crate::rgbd::supported_camera_frame(&self.sensor_frame))
                && !self.calibration_source.trim().is_empty(),
            "unverified sensor calibration"
        );
        Ok(())
    }
}
#[derive(Clone)]
pub struct MappingTarget {
    pub store: Arc<Mutex<Store>>,
    pub changes: watch::Sender<()>,
    pub status: watch::Sender<Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SensorReading {
    pub sensor_id: String,
    pub measurement: PointCloud,
}
impl auki_components::ContractType for SensorReading {
    const DATATYPE: &'static str = "galbot.sensor-reading";
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PointCloud {
    pub kind: String,
    pub width: usize,
    pub height: usize,
    pub point_step: usize,
    pub row_step: usize,
    pub is_bigendian: bool,
    pub fields: Vec<PointField>,
    pub data_base64: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PointField {
    pub name: String,
    pub offset: usize,
    pub datatype: u8,
    pub count: usize,
}

/// Preserve the measured local XYZ. No zero, NaN, max-range or inferred points.
/// Downsample deterministically across the entire scan; SDK's ray budget is bounded.
pub fn decode(reading: &SensorReading) -> Result<Vec<[f64; 3]>> {
    Ok(decode_colored(reading, 0)?.points)
}
pub struct DecodedScan {
    pub points: Vec<[f64; 3]>,
    pub colors: Vec<Option<[u8; 3]>>,
}
/// Select at most 384 measured points, shifting the stride's starting position
/// for each capture. Hashing the timestamp avoids locking to a sensor's regular
/// scan period; the same observation always selects the same points on replay.
pub fn decode_colored(reading: &SensorReading, timestamp_ns: u64) -> Result<DecodedScan> {
    ensure!(
        reading.sensor_id == "lidar.base" && reading.measurement.kind == "point_cloud",
        "unexpected LiDAR payload"
    );
    let c = &reading.measurement;
    ensure!(
        c.width > 0
            && c.height > 0
            && c.width <= 200_000
            && c.height <= 128
            && c.point_step >= 12
            && c.point_step <= 256,
        "invalid cloud dimensions"
    );
    let points = c
        .width
        .checked_mul(c.height)
        .ok_or_else(|| anyhow!("cloud size overflow"))?;
    ensure!(
        points <= 200_000
            && c.row_step >= c.width * c.point_step
            && c.row_step <= 8 * 1024 * 1024
            && c.row_step * c.height <= 8 * 1024 * 1024,
        "cloud stride/size invalid"
    );
    ensure!(
        c.data_base64.len() <= 12 * 1024 * 1024 && c.fields.len() <= 64,
        "cloud exceeds payload bounds"
    );
    let bytes = STANDARD.decode(&c.data_base64)?;
    ensure!(
        bytes.len() == c.row_step * c.height,
        "cloud data length mismatch"
    );
    let mut fields = Vec::new();
    for name in ["x", "y", "z"] {
        let matches: Vec<_> = c.fields.iter().filter(|f| f.name == name).collect();
        ensure!(matches.len() == 1, "missing/duplicate cloud {name}");
        let f = matches[0];
        let size = match f.datatype {
            7 => 4,
            8 => 8,
            _ => anyhow::bail!("XYZ must be float32/float64"),
        };
        ensure!(
            f.count == 1 && f.offset <= c.point_step - size,
            "XYZ field outside point stride"
        );
        fields.push((f.offset, size));
    }
    let color_field = |name: &str, size: usize| -> Result<Option<usize>> {
        let fs: Vec<_> = c.fields.iter().filter(|f| f.name == name).collect();
        ensure!(fs.len() <= 1, "duplicate color field");
        if let Some(f) = fs.first() {
            ensure!(
                f.count == 1
                    && f.offset + size <= c.point_step
                    && if size == 1 {
                        f.datatype == 2
                    } else {
                        matches!(f.datatype, 6 | 7)
                    },
                "invalid color layout"
            );
            Ok(Some(f.offset))
        } else {
            Ok(None)
        }
    };
    let separate = [
        color_field("r", 1)?,
        color_field("g", 1)?,
        color_field("b", 1)?,
    ];
    ensure!(
        separate.iter().all(Option::is_none) || separate.iter().all(Option::is_some),
        "incomplete RGB channels"
    );
    let packed = color_field("rgb", 4)?.or(color_field("rgba", 4)?);
    let mut colors = Vec::new();
    let stride = points.div_ceil(384);
    // SplitMix64's finalizer mixes even regularly spaced capture timestamps.
    let mut phase = timestamp_ns;
    phase = (phase ^ (phase >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    phase = (phase ^ (phase >> 27)).wrapping_mul(0x94d049bb133111eb);
    phase ^= phase >> 31;
    let offset = (phase % stride as u64) as usize;
    let mut hits = Vec::new();
    for i in (offset..points).step_by(stride) {
        let start = (i / c.width) * c.row_step + (i % c.width) * c.point_step;
        let p = std::array::from_fn(|axis| {
            let (offset, size) = fields[axis];
            let data = &bytes[start + offset..start + offset + size];
            if size == 4 {
                let a = data.try_into().unwrap();
                (if c.is_bigendian {
                    f32::from_be_bytes(a)
                } else {
                    f32::from_le_bytes(a)
                }) as f64
            } else {
                let a = data.try_into().unwrap();
                if c.is_bigendian {
                    f64::from_be_bytes(a)
                } else {
                    f64::from_le_bytes(a)
                }
            }
        });
        let r = p.iter().map(|v| v * v).sum::<f64>();
        if p.iter().all(|v| v.is_finite()) && (0.09..25.).contains(&r) {
            hits.push(p);
            colors.push(if let [Some(r), Some(g), Some(b)] = separate {
                Some([bytes[start + r], bytes[start + g], bytes[start + b]])
            } else if let Some(offset) = packed {
                let raw = bytes[start + offset..start + offset + 4]
                    .try_into()
                    .unwrap();
                let n = if c.is_bigendian {
                    u32::from_be_bytes(raw)
                } else {
                    u32::from_le_bytes(raw)
                };
                Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
            } else {
                None
            });
        }
    }
    ensure!(!hits.is_empty(), "no valid LiDAR returns within 0.3–5 m");
    Ok(DecodedScan {
        points: hits,
        colors,
    })
}

pub fn binding(
    producer: &OutputManifest,
    source: ProductReference,
    clock: &ClockReference,
    config: &LidarConfig,
) -> Result<SensorBinding> {
    config.validate()?;
    ensure!(
        config.sensor_frame == "lidar_base_link",
        "unexpected LiDAR calibration frame"
    );
    ensure!(
        producer.peer_id == source.peer_id
            && producer.component_id == "lidar.base"
            && producer.payload.schema() == "galbot.sensor-reading/v1",
        "unexpected LiDAR producer"
    );
    ensure!(
        &producer.clock == clock,
        "LiDAR and odometry capture clocks differ"
    );
    let frame = FrameRegistryEntry::ros_body(&source.peer_id, &config.sensor_frame);
    ensure!(
        producer.spatial_frame.as_ref().map(|f| &f.definition) == Some(&frame),
        "LiDAR frame differs from configured extrinsic"
    );
    Ok(SensorBinding {
        sensor_product: source,
        sensor_frame: frame,
        observation_clock: clock.clone(),
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Watermark {
    pub source: ProductReference,
    pub sequence: u64,
    pub timestamp_ns: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub schema: u32,
    pub portals: MapSnapshot,
    pub voxels: VoxelSnapshot,
    pub watermarks: Vec<Watermark>,
    pub alignment_key: String,
    pub calibration: LidarConfig,
    #[serde(default)]
    pub calibrations: Vec<(ProductReference, SensorCalibration)>,
    #[serde(default)]
    pub inputs: Vec<MappingInput>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MappingInput {
    #[serde(alias = "revision")]
    pub sequence: usize,
    pub scan: PreparedScan,
}
impl auki_components::ContractType for MappingInput {
    const DATATYPE: &'static str = "spatial-harness.mapping-input/v1";
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedScan {
    pub source: SensorBinding,
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub hit_points_m: Vec<[f64; 3]>,
    #[serde(default)]
    pub hit_colors_srgb: Vec<Option<[u8; 3]>>,
    pub sensor_to_home: Pose,
    pub alignment_key: String,
    pub calibration: LidarConfig,
}
pub struct Scan {
    pub source: SensorBinding,
    pub observation: Observation<SensorReading>,
    pub sensor_to_home: Pose,
    pub alignment_key: String,
    pub calibration: LidarConfig,
}
impl Store {
    pub fn set_map_owner(&mut self, owner: &str) -> Result<()> {
        if self.built.voxels.integrated_observations > 0 {
            ensure!(
                self.built.voxels.definition.registry.peer_id == owner,
                "saved scene belongs to a different mapper peer"
            );
            return Ok(());
        }
        let runtime = auki_components::ComponentRuntime::new(owner);
        let mut definition = self.built.voxels.definition.clone();
        definition.registry.peer_id = owner.into();
        let clock = crate::model::clock_ref(&crate::model::clock_entry(owner, "mapper"));
        let component = auki_voxel_map::VoxelMapComponent::new(
            &runtime,
            "runner-voxel-mapper",
            "runner-voxels",
            clock,
            definition,
            || 1,
        )?;
        self.built.voxels = (*component
            .product()
            .latest_existing()?
            .ok_or_else(|| anyhow!("empty mapper snapshot missing"))?
            .payload)
            .clone();
        Ok(())
    }
    pub(crate) fn restore_runner_voxels(&mut self) -> Result<()> {
        let path = self.path.with_extension("voxels.json");
        if !path.exists() {
            return Ok(());
        }
        ensure!(
            std::fs::metadata(&path)?.len() <= 32 * 1024 * 1024,
            "runner checkpoint too large"
        );
        let mut saved: Checkpoint = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(
            saved.schema == 1
                && crate::conventions::matching_portals(&saved.portals, &self.built.portals)?,
            "saved runner map does not match supplied portals"
        );
        // Persist a lossless rebase: keep every voxel cell/evidence/color byte,
        // translating its lattice origin rather than resampling the 10 cm grid.
        if saved.portals.scenegraph.map.frame.id == crate::domain_portals::FRAME
            && self.built.portals.scenegraph.map.frame.id == FRAME
        {
            let old_frame = saved.voxels.definition.frame.clone();
            let mut definition = saved.voxels.definition.clone();
            crate::conventions::set_definition_frame(
                &mut definition,
                &old_frame.peer_id,
                &self.built.portals,
            )?;
            definition.portal_snapshot = self.built.voxels.definition.portal_snapshot.clone();
            definition.grid_origin_m = crate::conventions::point(
                saved.voxels.definition.grid_origin_m,
                &old_frame,
                &definition.frame,
            )?;
            if let Some(last) = &mut saved.voxels.latest_observation {
                last.pose.sensor_to_map.translation = crate::conventions::point(
                    last.pose.sensor_to_map.translation,
                    &old_frame,
                    &definition.frame,
                )?;
                last.pose.sensor_to_map.to_frame_id = FRAME.into();
                last.pose.portal_snapshot = definition.portal_snapshot.clone();
            }
            for input in &mut saved.inputs {
                input.scan.sensor_to_home = crate::conventions::pose(
                    &input.scan.sensor_to_home,
                    &old_frame,
                    &definition.frame,
                )?;
            }
            saved.voxels.definition = definition;
            saved.portals = self.built.portals.clone();
            saved.voxels.accumulator()?;
            let path = self.path.with_extension("voxels.json");
            let backup = self.path.with_extension("before-ucof-origin.voxels.json");
            if !backup.exists() {
                std::fs::copy(&path, &backup)?;
            }
            crate::model::persist(&path, &saved)?;
        }
        saved.voxels.accumulator()?;
        saved.calibration.validate()?;
        ensure!(
            saved.voxels.definition.portal_map == saved.portals.scenegraph.map
                && saved.voxels.definition.frame.frame_id == saved.portals.scenegraph.map.frame.id,
            "checkpoint frame mismatch"
        );
        if saved.portals.scenegraph.map.frame.id != self.built.portals.scenegraph.map.frame.id {
            ensure!(
                self.built.portals.scenegraph.map.frame.id == crate::conventions::RUNNER_FRAME,
                "unexpected checkpoint migration"
            );
            ensure!(
                saved.voxels.definition.registry.peer_id
                    == self.built.voxels.definition.registry.peer_id,
                "checkpoint owner mismatch"
            );
            let mut definition = saved.voxels.definition.clone();
            crate::conventions::set_definition_frame(
                &mut definition,
                &self.built.voxels.definition.registry.peer_id,
                &self.built.portals,
            )?;
            definition.portal_snapshot = self.built.voxels.definition.portal_snapshot.clone();
            saved.voxels = crate::conventions::voxel_checkpoint(&saved.voxels, definition)?;
            saved.portals = self.built.portals.clone();
            let path = self.path.with_extension("voxels.json");
            let backup = self.path.with_extension("opengl-backup.json");
            if !backup.exists() {
                std::fs::copy(&path, &backup)?;
            }
            crate::model::persist(&path, &saved)?;
        }
        self.publish_runner_checkpoint(saved)?;
        Ok(())
    }
    fn publish_runner_checkpoint(&mut self, saved: Checkpoint) -> Result<()> {
        self.built.view.voxels = crate::model::occupied(&saved.voxels)?;
        self.built.view.integrated_observations = saved.voxels.integrated_observations;

        self.built.view.sources = std::collections::BTreeMap::from([(
            "galbot".into(),
            saved.voxels.integrated_observations as usize,
        )]);
        let mut frames = vec![saved.voxels.definition.frame.clone()];
        frames.extend(
            saved
                .voxels
                .definition
                .sources
                .iter()
                .map(|s| s.sensor_frame.clone()),
        );
        self.built.registries["frames"] = json!(frames);
        self.built.registries["source_bindings"] = json!(saved.voxels.definition.sources);
        self.built.voxels = saved.voxels.clone();
        self.live = Some(saved);
        Ok(())
    }
    pub fn integrate_runner(&mut self, scan: Scan) -> Result<bool> {
        ensure!(
            scan.observation.output.peer_id == scan.source.sensor_product.peer_id
                && scan.observation.output.component_id == "lidar.base",
            "LiDAR observation owner mismatch"
        );
        let decoded = decode_colored(&scan.observation.payload, scan.observation.timestamp_ns)?;
        let prepared = PreparedScan {
            source: scan.source,
            sequence: scan.observation.sequence,
            timestamp_ns: scan.observation.timestamp_ns,
            hit_points_m: decoded.points,
            hit_colors_srgb: decoded.colors,
            sensor_to_home: scan.sensor_to_home,
            alignment_key: scan.alignment_key,
            calibration: scan.calibration,
        };
        self.integrate_prepared(prepared)
    }
    pub fn mapping_inputs(&self) -> &[MappingInput] {
        self.live
            .as_ref()
            .map(|s| s.inputs.as_slice())
            .unwrap_or(&[])
    }
    pub fn integrate_prepared(&mut self, scan: PreparedScan) -> Result<bool> {
        let retained_input = scan.clone();
        ensure!(!self.demo, "real runner scans cannot enter a demo map");
        scan.sensor_to_home.validate()?;
        scan.calibration.validate()?;
        let mut saved = if let Some(current) = &self.live {
            ensure!(
                current.alignment_key == scan.alignment_key,
                "runner calibration changed; existing voxels need an explicit rebuild"
            );
            let mut saved = current.clone();
            if saved.calibrations.is_empty() {
                saved.calibrations = saved
                    .voxels
                    .definition
                    .sources
                    .iter()
                    .map(|s| (s.sensor_product.clone(), saved.calibration.clone()))
                    .collect();
            }
            saved
        } else {
            let mut voxels = self.built.voxels.clone();
            voxels.definition.sources = vec![scan.source.clone()];
            Checkpoint {
                schema: 1,
                portals: self.built.portals.clone(),
                voxels,
                watermarks: vec![],
                alignment_key: scan.alignment_key.clone(),
                calibration: scan.calibration.clone(),
                calibrations: vec![],
                inputs: vec![],
            }
        };
        if let Some(previous) = saved
            .watermarks
            .iter()
            .find(|w| w.source == scan.source.sensor_product)
        {
            if scan.sequence <= previous.sequence {
                return Ok(false);
            }
            ensure!(
                scan.timestamp_ns > previous.timestamp_ns,
                "nonmonotonic LiDAR timestamp"
            );
        }
        crate::voxel_color::enable(&mut saved.voxels.definition);
        let source = scan.source.sensor_product.clone();
        ensure!(
            scan.source.sensor_frame.frame_id == scan.calibration.sensor_frame,
            "sensor calibration frame mismatch"
        );
        if let Some((_, previous)) = saved.calibrations.iter().find(|(p, _)| p == &source) {
            ensure!(
                previous.sensor_frame == scan.calibration.sensor_frame
                    && previous.calibration_source == scan.calibration.calibration_source
                    && (crate::rgbd::supported_camera_frame(&scan.calibration.sensor_frame)
                        || serde_json::to_value(&previous.sensor_to_base)?
                            == serde_json::to_value(&scan.calibration.sensor_to_base)?),
                "sensor calibration changed; explicit rebuild required"
            );
        } else {
            ensure!(saved.calibrations.len() < 16, "too many sensor sources");
            saved
                .calibrations
                .push((source.clone(), scan.calibration.clone()));
        }
        if let Some(previous) = saved
            .voxels
            .definition
            .sources
            .iter()
            .find(|s| s.sensor_product == source)
        {
            ensure!(previous == &scan.source, "sensor binding changed");
        } else {
            ensure!(
                saved.voxels.definition.sources.len() < 16
                    && saved
                        .voxels
                        .definition
                        .sources
                        .iter()
                        .all(|s| s.observation_clock == scan.source.observation_clock),
                "sensor clock mismatch or source limit exceeded"
            );
            saved.voxels.definition.sources.push(scan.source.clone());
        }
        let map_pose = crate::conventions::pose(
            &scan.sensor_to_home,
            &crate::conventions::registry("home", FRAME)?,
            &saved.voxels.definition.frame,
        )?;
        let [x, y, z, w] = map_pose.rotation_xyzw;
        let observation = DepthObservation {
            source: source.clone(),
            sequence: scan.sequence,
            timestamp_ns: scan.timestamp_ns,
            clock: scan.source.observation_clock.clone(),
            sensor_frame_id: scan.source.sensor_frame.frame_id.clone(),
            hit_points_m: scan.hit_points_m,
            pose: TimedSensorPose {
                timestamp_ns: scan.timestamp_ns,
                clock: scan.source.observation_clock,
                sensor_to_map: RigidTransform {
                    from_frame_id: scan.source.sensor_frame.frame_id,
                    to_frame_id: saved.voxels.definition.frame.frame_id.clone(),
                    translation: map_pose.translation,
                    rotation_wxyz: [w, x, y, z],
                },
                portal_snapshot: saved.voxels.definition.portal_snapshot.clone(),
            },
        };
        let mapper = PortalVoxelMapper::new(saved.voxels.definition.clone())?;
        let mut update = mapper.map_observation(&observation)?;
        let t = &observation.pose.sensor_to_map;
        let [w, x, y, z] = t.rotation_wxyz;
        crate::voxel_color::add(
            &mut update,
            &observation.hit_points_m,
            &scan.hit_colors_srgb,
            &auki_datatypes::pose::SpatialTransform {
                translation: Some(auki_datatypes::pose::Vec3 {
                    x: t.translation[0] - saved.voxels.definition.grid_origin_m[0],
                    y: t.translation[1] - saved.voxels.definition.grid_origin_m[1],
                    z: t.translation[2] - saved.voxels.definition.grid_origin_m[2],
                }),
                orientation: Some(auki_datatypes::pose::Quat { x, y, z, w }),
            },
            saved.voxels.definition.grid().voxel_size_m.0,
        )?;
        let mut accumulator = saved.voxels.accumulator()?;
        accumulator.apply(&update)?;
        saved.voxels.checkpoint = accumulator.checkpoint_update().encode_to_vec();
        saved.voxels.integrated_observations += 1;
        saved.voxels.latest_observation = Some((&observation).into());
        saved.voxels.accumulator()?;
        saved.watermarks.retain(|w| w.source != source);
        saved.watermarks.push(Watermark {
            source,
            sequence: scan.sequence,
            timestamp_ns: scan.timestamp_ns,
        });
        // Persist before exposing a updated canonical state. Checkpoints are replacements,
        // never additive deltas re-applied after restart.
        saved.inputs.push(MappingInput {
            sequence: saved.voxels.integrated_observations as usize,
            scan: retained_input,
        });
        if saved.inputs.len() > 256 {
            saved.inputs.drain(..saved.inputs.len() - 256);
        }
        crate::model::persist(&self.path.with_extension("voxels.json"), &saved)?;
        self.publish_runner_checkpoint(saved)?;
        Ok(true)
    }
}

pub async fn fetch(
    client: &auki_component_protocol::ComponentProtocolClient,
    peer: auki_sdk::PeerId,
    route: auki_sdk::Multiaddr,
    product: Option<ProductReference>,
) -> Result<auki_component_protocol::RemoteObservations<SensorReading>> {
    let Some(product) = product else {
        std::future::pending::<()>().await;
        unreachable!()
    };
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    Ok(tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.observations_exact(
            peer,
            route,
            auki_component_protocol::ObservationRequest {
                product,
                selection: auki_component_protocol::ObservationSelection::LatestExisting,
            },
        ),
    )
    .await??)
}
/// Only stationary scans in this first slice. Vendor per-point timestamps are
/// explicitly opaque, so do not invent a clock conversion or deskew moving scans.
pub fn place(
    sample: &auki_component_protocol::RemoteObservations<SensorReading>,
    odom: &auki_components::RetainedProduct<auki_odometry::PoseUpdate>,
    alignment: &crate::runner_pose::MapAlignment,
    alignment_time: u64,
    config: &LidarConfig,
) -> Result<Option<Scan>> {
    ensure!(
        sample.gap.is_none() && sample.end.is_none(),
        "LiDAR Product ended or has a retention gap"
    );
    let o = sample
        .observations
        .last()
        .ok_or_else(|| anyhow!("LiDAR Product has no observations"))?;
    ensure!(
        o.output == sample.producer.reference(),
        "LiDAR observation producer mismatch"
    );
    ensure!(
        o.timestamp_ns >= alignment_time,
        "LiDAR scan predates map alignment"
    );
    let source = ProductReference {
        peer_id: sample.product.peer_id.clone(),
        product_id: sample.product.product_id.clone(),
        manifest_hash: sample.product_manifest_hash.clone(),
    };
    let source = binding(
        &sample.producer,
        source,
        &alignment.transform.contract.clock,
        config,
    )?;
    decode_colored(&o.payload, o.timestamp_ns)?;
    let Some(pose) = capture_pose(odom, alignment, o.timestamp_ns)? else {
        return Ok(None);
    };
    Ok(Some(Scan {
        source,
        observation: o.clone(),
        sensor_to_home: crate::runner_pose::compose(&pose, &config.sensor_to_base),
        alignment_key: serde_json::to_string(alignment)?,
        calibration: config.clone(),
    }))
}

/// Shared capture-time placement for physical Galbot sensors. Until deskew is
/// available, require a stationary base around the measurement timestamp.
pub fn capture_pose(
    odom: &auki_components::RetainedProduct<auki_odometry::PoseUpdate>,
    alignment: &crate::runner_pose::MapAlignment,
    timestamp_ns: u64,
) -> Result<Option<Pose>> {
    use auki_odometry::{PoseContract, PoseError, PoseQuery, PoseUpdate, Tracking, pose_at};
    let a = &alignment.transform.contract;
    let contract = PoseContract {
        from_frame: FrameRegistryEntry::ros_body(&a.from_frame.peer_id, "robot.base"),
        to_frame: a.from_frame.clone(),
        clock: a.clock.clone(),
        session_id: a.session_id.clone(),
    };
    let query = |t| {
        pose_at(
            odom,
            &PoseQuery {
                contract: contract.clone(),
                timestamp_ns: t,
                max_gap_ns: 200_000_000,
            },
        )
    };
    let mut poses = Vec::new();
    for t in [
        timestamp_ns.saturating_sub(150_000_000),
        timestamp_ns,
        timestamp_ns.saturating_add(150_000_000),
    ] {
        match query(t) {
            Ok(p) => poses.push(p.pose),
            Err(PoseError::OutsideHistory) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    }
    let displacement = poses[0]
        .translation
        .iter()
        .zip(poses[2].translation)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt();
    let dot = poses[0]
        .rotation_xyzw
        .iter()
        .zip(poses[2].rotation_xyzw)
        .map(|(a, b)| a * b)
        .sum::<f64>()
        .abs()
        .min(1.);
    ensure!(
        displacement < 0.01 && 2. * dot.acos() < 0.0174533,
        "Skipping moving scan; motion compensation is not yet calibrated"
    );
    let pose = crate::runner_pose::world_pose(
        &PoseUpdate {
            contract,
            tracking: Tracking::Valid {
                pose: poses[1].clone(),
            },
        },
        alignment,
        &a.from_frame.peer_id,
    )?
    .ok_or_else(|| anyhow!("map alignment tracking lost"))?;
    Ok(Some(crate::conventions::pose(
        &pose,
        &FrameRegistryEntry::opengl("home", crate::domain_portals::FRAME),
        &FrameRegistryEntry::opengl("home", crate::model::FRAME),
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reading(big: bool, points: &[[f32; 3]]) -> SensorReading {
        let mut bytes = Vec::new();
        for p in points {
            for n in p {
                bytes.extend(if big {
                    n.to_be_bytes()
                } else {
                    n.to_le_bytes()
                });
            }
            bytes.extend([0; 4]);
        }
        // Row padding must not be interpreted as an extra point.
        bytes.extend([0; 8]);
        SensorReading {
            sensor_id: "lidar.base".into(),
            measurement: PointCloud {
                kind: "point_cloud".into(),
                width: points.len(),
                height: 1,
                point_step: 16,
                row_step: points.len() * 16 + 8,
                is_bigendian: big,
                fields: ["x", "y", "z"]
                    .iter()
                    .enumerate()
                    .map(|(i, n)| PointField {
                        name: (*n).into(),
                        offset: i * 4,
                        datatype: 7,
                        count: 1,
                    })
                    .collect(),
                data_base64: STANDARD.encode(bytes),
            },
        }
    }
    #[test]
    fn decodes_endianness_stride_and_rejects_nonreturns_and_malformed_clouds() {
        for big in [true, false] {
            let input = reading(
                big,
                &[
                    [1., 2., 0.5],
                    [0., 0., 0.],
                    [f32::NAN, 1., 1.],
                    [8., 0., 0.],
                ],
            );
            assert_eq!(decode(&input).unwrap(), vec![[1., 2., 0.5]]);
            let mut bad = input.clone();
            bad.measurement.fields[0].offset = 15;
            assert!(decode(&bad).is_err());
            let mut bad = input;
            bad.measurement.row_step += 1;
            assert!(decode(&bad).is_err());
        }
    }
    #[test]
    fn repeated_structured_scans_gain_coverage_without_more_rays_or_color_mismatch() {
        use std::collections::HashSet;
        // A stationary 96-channel scanner: identical raw geometry every scan.
        let points: Vec<[f32; 3]> = (0..86_400)
            .map(|i| {
                let azimuth = (i / 96) as f32 * std::f32::consts::TAU / 900.;
                let elevation = (i % 96) as f32 * std::f32::consts::FRAC_PI_2 / 96.;
                [
                    3. * elevation.cos() * azimuth.cos(),
                    3. * elevation.cos() * azimuth.sin(),
                    3. * elevation.sin(),
                ]
            })
            .collect();
        for big in [false, true] {
            let mut input = reading(big, &points);
            input.measurement.fields.push(PointField {
                name: "rgb".into(),
                offset: 12,
                datatype: 6,
                count: 1,
            });
            let mut bytes = STANDARD.decode(&input.measurement.data_base64).unwrap();
            for i in 0..points.len() {
                let color = i as u32;
                bytes[i * 16 + 12..i * 16 + 16].copy_from_slice(&if big {
                    color.to_be_bytes()
                } else {
                    color.to_le_bytes()
                });
            }
            input.measurement.data_base64 = STANDARD.encode(bytes);
            let cell = |p: &[f64; 3]| p.map(|v| (v / 0.1).floor() as i32);
            let fixed = decode_colored(&input, 0).unwrap();
            let fixed_cells: HashSet<_> = fixed.points.iter().map(cell).collect();
            let mut varied_cells = HashSet::new();
            for capture in 1..=10 {
                let timestamp = 1_000_000_000 * capture;
                let scan = decode_colored(&input, timestamp).unwrap();
                let replay = decode_colored(&input, timestamp).unwrap();
                assert_eq!(scan.points, replay.points);
                assert_eq!(scan.colors, replay.colors);
                assert!(scan.points.len() <= 384);
                assert_eq!(scan.points.len(), scan.colors.len());
                for (point, color) in scan.points.iter().zip(&scan.colors) {
                    let [r, g, b] = color.unwrap();
                    let index = ((r as usize) << 16) | ((g as usize) << 8) | b as usize;
                    assert_eq!(*point, points[index].map(f64::from));
                    varied_cells.insert(cell(point));
                }
            }
            assert!(
                varied_cells.len() > 2 * fixed_cells.len(),
                "varying scan selection must improve stationary structured-cloud coverage"
            );
        }
    }
    #[test]
    fn shifted_sampling_handles_small_and_nondivisible_cloud_sizes() {
        for count in [1, 383, 384, 385, 769, 19_968, 86_401] {
            let input = reading(false, &vec![[1., 2., 0.5]; count]);
            for timestamp in [0, 1, 1_000_000_000, u64::MAX] {
                let scan = decode_colored(&input, timestamp).unwrap();
                assert!(!scan.points.is_empty() && scan.points.len() <= 384);
                if count <= 384 {
                    assert_eq!(scan.points.len(), count);
                }
            }
        }
    }
    fn scan(sequence: u64) -> Scan {
        let config = LidarConfig {
            sensor_frame: "lidar_base_link".into(),
            sensor_to_base: Pose {
                translation: [0.; 3],
                rotation_xyzw: [0., 0., 0., 1.],
            },
            calibration_source: "test fixture".into(),
        };
        let peer = "test-runner";
        let source = ProductReference {
            peer_id: peer.into(),
            product_id: "lidar.base-history".into(),
            manifest_hash: "test-product".into(),
        };
        Scan {
            source: SensorBinding {
                sensor_product: source,
                sensor_frame: FrameRegistryEntry::ros_body(peer, "lidar_base_link"),
                observation_clock: crate::model::clock_ref(&crate::model::clock_entry(
                    peer, "capture",
                )),
            },
            observation: Observation {
                output: auki_components::OutputReference {
                    peer_id: peer.into(),
                    component_id: "lidar.base".into(),
                    component_manifest_hash: "component".into(),
                    slot: "samples".into(),
                    output_id: "lidar-output".into(),
                    manifest_hash: "output".into(),
                },
                sequence,
                timestamp_ns: sequence * 1_000_000_000,
                payload: Arc::new(reading(
                    false,
                    &[[1., 0., 0.], [1., 0.2, 0.], [1., 0., 0.2]],
                )),
            },
            sensor_to_home: Pose {
                translation: [-4., 0.3, 4.],
                rotation_xyzw: [0., 0., 0., 1.],
            },
            alignment_key: "verified-test-alignment".into(),
            calibration: config,
        }
    }
    #[test]
    fn live_map_is_clean_and_real_scans_persist_without_replay_or_fixture_contamination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.json");
        let mut store = Store::open_live(&path).unwrap();
        assert_eq!(store.built.view.portals.len(), 13);
        assert!(store.built.view.voxels.is_empty());
        assert!(store.built.view.objects.is_empty());
        assert!(store.built.view.synthetic_layout.is_none());
        let c =
            serde_json::from_value(crate::model::fixtures().unwrap()["contributions"][0].clone())
                .unwrap();
        assert!(store.apply(crate::model::Event::Contribution(c)).is_err());
        assert!(store.integrate_runner(scan(1)).unwrap());
        assert!(!store.built.view.voxels.is_empty());
        assert_eq!(store.built.view.integrated_observations, 1);
        let checkpoint = store.built.voxels.clone();
        drop(store);
        let mut restored = Store::open_live(&path).unwrap();
        assert_eq!(restored.built.voxels, checkpoint);
        assert!(!restored.integrate_runner(scan(1)).unwrap());
        assert_eq!(restored.built.voxels, checkpoint);
        let mut changed = scan(2);
        changed.alignment_key = "different-calibration".into();
        assert!(restored.integrate_runner(changed).is_err());
        assert_eq!(restored.built.voxels, checkpoint);
        assert!(restored.integrate_runner(scan(2)).unwrap());
    }
    #[test]
    fn failed_checkpoint_write_does_not_publish_geometry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.json");
        let mut store = Store::open_live(&path).unwrap();
        std::fs::create_dir(path.with_extension("voxels.json")).unwrap();
        assert!(store.integrate_runner(scan(1)).is_err());
        assert!(store.built.view.voxels.is_empty());
        assert_eq!(store.built.view.integrated_observations, 0);
    }
    #[test]
    fn scan_placement_requires_capture_time_history_and_rejects_motion_or_wrong_clock() {
        use auki_components::*;
        use auki_odometry::*;
        use auki_registry::CoordinateConvention;
        let peer = "test-runner";
        let runtime = ComponentRuntime::new(peer);
        let clock = crate::model::clock_ref(&crate::model::clock_entry(peer, "session"));
        let contract = PoseContract {
            from_frame: FrameRegistryEntry::ros_body(peer, "robot.base"),
            to_frame: FrameRegistryEntry::ros_body(peer, "robot.odometry.session"),
            clock: clock.clone(),
            session_id: "session".into(),
        };
        let mut odom = OdometryComponent::new(&runtime, "odometry", contract.clone()).unwrap();
        let history =
            capture_pose_history(&runtime, "poses", &odom, BufferLimits::entries(100)).unwrap();
        let identity = Pose {
            translation: [0.; 3],
            rotation_xyzw: [0., 0., 0., 1.],
        };
        let alignment = crate::runner_pose::MapAlignment {
            domain_id: crate::domain_portals::DOMAIN.into(),
            map_id: "calibrated".into(),
            transform: PoseUpdate {
                contract: PoseContract {
                    from_frame: contract.to_frame,
                    to_frame: FrameRegistryEntry::in_convention(
                        peer,
                        format!("domain.{}.zup", crate::domain_portals::DOMAIN),
                        CoordinateConvention::ZUpRightForward,
                    ),
                    clock: clock.clone(),
                    session_id: "session".into(),
                },
                tracking: Tracking::Valid {
                    pose: identity.clone(),
                },
            },
        };
        let component = runtime
            .component(
                ComponentSpec::new("lidar.base").observable(ObservableContract {
                    name: "samples".into(),
                    datatype: "galbot.sensor-reading".into(),
                    schema: "galbot.sensor-reading/v1".into(),
                    access: vec![ObservationAccess::FollowNew],
                    exposure: Exposure::Cluster,
                }),
            )
            .unwrap();
        let output = component
            .configured_observable::<SensorReading>(
                ConfiguredObservableSpec::new(
                    "samples",
                    "lidar",
                    clock,
                    PayloadContract::Structured(StructuredPayloadContract {
                        modality: "point_cloud".into(),
                        datatype: "galbot.sensor-reading".into(),
                        schema: "galbot.sensor-reading/v1".into(),
                        observes: "lidar.base".into(),
                        unit: Some("meters".into()),
                    }),
                )
                .in_registered_frame(FrameRegistryEntry::ros_body(peer, "lidar_base_link")),
            )
            .unwrap();
        component.expose().unwrap();
        let capture = runtime
            .capture_buffer(
                "lidar.base-history",
                &output,
                BufferLimits::entries(2),
                |_| 1024,
            )
            .unwrap();
        let (observation, _) = output
            .publish(1_200_000_000, Arc::new(reading(false, &[[1., 0., 0.]])))
            .unwrap();
        let product = capture.product();
        let mut sample = auki_component_protocol::RemoteObservations {
            product: product.manifest.clone(),
            product_manifest_hash: product.manifest_hash.clone(),
            producer: product.producer.clone(),
            observations: vec![observation],
            gap: None,
            end: None,
        };
        let config = scan(1).calibration;
        // A latest pose before the scan is insufficient. Never extrapolate.
        odom.publish(
            1_000_000_000,
            Tracking::Valid {
                pose: identity.clone(),
            },
        )
        .unwrap();
        assert!(
            place(&sample, &history.product(), &alignment, 0, &config)
                .unwrap()
                .is_none()
        );
        for t in [1_100_000_000, 1_200_000_000, 1_300_000_000, 1_400_000_000] {
            odom.publish(
                t,
                Tracking::Valid {
                    pose: identity.clone(),
                },
            )
            .unwrap();
        }
        assert!(
            place(&sample, &history.product(), &alignment, 0, &config)
                .unwrap()
                .is_some()
        );
        sample.producer.clock.id = "wrong-clock".into();
        assert!(place(&sample, &history.product(), &alignment, 0, &config).is_err());
        sample.producer = product.producer.clone();
        sample.observations[0].timestamp_ns = 1_400_000_000;
        let mut moved = identity.clone();
        moved.translation[0] = 0.2;
        odom.publish(
            1_500_000_000,
            Tracking::Valid {
                pose: moved.clone(),
            },
        )
        .unwrap();
        odom.publish(1_600_000_000, Tracking::Valid { pose: moved })
            .unwrap();
        assert!(place(&sample, &history.product(), &alignment, 0, &config).is_err());
    }
}

#[cfg(test)]
mod color_and_conversion_tests {
    use super::*;
    #[test]
    fn packed_rgb_preserves_color_and_filters_with_matching_points() {
        for big in [false, true] {
            let fields = [("x", 0, 7), ("y", 4, 7), ("z", 8, 7), ("rgba", 12, 6)]
                .map(|(name, offset, datatype)| PointField {
                    name: name.into(),
                    offset,
                    datatype,
                    count: 1,
                })
                .to_vec();
            let mut bytes = Vec::new();
            for (x, rgb) in [(1f32, 0xff804020u32), (0., 0xff00ff00)] {
                for n in [x, 0., 0.] {
                    bytes.extend(if big {
                        n.to_be_bytes()
                    } else {
                        n.to_le_bytes()
                    });
                }
                bytes.extend(if big {
                    rgb.to_be_bytes()
                } else {
                    rgb.to_le_bytes()
                });
            }
            let reading = SensorReading {
                sensor_id: "lidar.base".into(),
                measurement: PointCloud {
                    kind: "point_cloud".into(),
                    width: 2,
                    height: 1,
                    point_step: 16,
                    row_step: 32,
                    is_bigendian: big,
                    fields,
                    data_base64: STANDARD.encode(bytes),
                },
            };
            let decoded = decode_colored(&reading, 0).unwrap();
            assert_eq!(decoded.points, vec![[1., 0., 0.]]);
            assert_eq!(decoded.colors, vec![Some([128, 64, 32])]);
        }
    }
    #[test]
    fn colored_map_migration_preserves_all_evidence_and_restart() -> Result<()> {
        use crate::conventions;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("robot.json");
        let mut home = Store::open_live(&path)?;
        home.set_map_owner("robot")?;
        let portals = home.built.portals.clone();
        let scan = PreparedScan {
            source: SensorBinding {
                sensor_product: ProductReference {
                    peer_id: "robot".into(),
                    product_id: "lidar".into(),
                    manifest_hash: "fixture".into(),
                },
                sensor_frame: FrameRegistryEntry::ros_body("robot", "lidar_base_link"),
                observation_clock: crate::model::test_clock("robot"),
            },
            sequence: 1,
            timestamp_ns: 1,
            hit_points_m: vec![[1., 0., 0.], [1., 0., 0.]],
            hit_colors_srgb: vec![Some([255, 0, 0]), Some([0, 0, 255])],
            sensor_to_home: Pose {
                translation: [-0.235, 0.335, -0.435],
                rotation_xyzw: [0., 0., 0., 1.],
            },
            alignment_key: "fixture".into(),
            calibration: LidarConfig {
                sensor_frame: "lidar_base_link".into(),
                sensor_to_base: Pose {
                    translation: [0.; 3],
                    rotation_xyzw: [0., 0., 0., 1.],
                },
                calibration_source: "fixture".into(),
            },
        };
        assert!(home.integrate_prepared(scan.clone())?);
        assert_eq!(home.built.view.voxels[0].linear_rgb, Some([0.5, 0., 0.5]));
        let before = home.built.voxels.clone();
        drop(home);
        let mut robot = Store::open_mapper(&path, &portals, "robot")?;
        assert!(path.with_extension("opengl-backup.json").exists());
        assert_eq!(robot.built.voxels.integrated_observations, 1);
        assert_eq!(
            robot.built.voxels.definition.frame.frame_id,
            conventions::RUNNER_FRAME
        );
        assert!(!robot.integrate_prepared(scan.clone())?);
        let roundtrip =
            conventions::voxel_checkpoint(&robot.built.voxels, before.definition.clone())?;
        assert_eq!(
            roundtrip.accumulator()?.checkpoint_update(),
            before.accumulator()?.checkpoint_update()
        );
        drop(robot);
        let mut restored = Store::open_mapper(&path, &portals, "robot")?;
        let mut next = scan;
        next.sequence = 2;
        next.timestamp_ns = 2;
        assert!(restored.integrate_prepared(next)?);
        assert_eq!(
            restored.built.view.voxels[0].linear_rgb,
            Some([0.5, 0., 0.5])
        );
        assert_eq!(restored.built.view.integrated_observations, 2);
        let mut wrong = portals.clone();
        wrong.scenegraph.map.frame.convention =
            Some(auki_registry::CoordinateConvention::ZUpRightForward);
        assert!(conventions::portals(&wrong, conventions::RUNNER_FRAME).is_err());
        Ok(())
    }
}
