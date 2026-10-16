//! Shared sensor adapters use the runner's Component runtime and authenticated endpoint.
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;
use auki_registry::FrameRegistryEntry;
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;
type Result<T> = std::result::Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
use serde::Serialize;

pub fn history_limits() -> BufferLimits {
    BufferLimits {
        max_entries: Some(10_000),
        max_bytes: Some(16 * 1024 * 1024),
        target_duration: Some(Duration::from_secs(10)),
    }
}

#[derive(Clone, Deserialize)]
pub struct SensorSpec {
    pub id: String,
    pub clock: ClockReference,
    pub frame_definition: Option<FrameRegistryEntry>,
    pub payload: PayloadContract,
    pub metadata: serde_json::Value,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SensorReading {
    pub sensor_id: String,
    pub measurement: Measurement,
}
impl ContractType for SensorReading {
    // Shared historical wire identity; Bracketbot still uses this contract.
    const DATATYPE: &'static str = "galbot.sensor-reading";
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Measurement {
    Joint {
        name: String,
        position_rad: f64,
        velocity_rad_s: f64,
        effort_nm: f64,
        current_a: f64,
        acceleration_rad_s2: f64,
    },
    PointCloud {
        width: u32,
        height: u32,
        point_step: u32,
        row_step: u32,
        is_bigendian: bool,
        is_dense: bool,
        fields: Vec<PointField>,
        data_base64: String,
    },
    Depth {
        width: u32,
        height: u32,
        encoding: String,
        counts_per_meter: f64,
        data_base64: String,
    },
    Imu {
        acceleration_m_s2: [f64; 3],
        angular_velocity_rad_s: [f64; 3],
    },
    Wrench {
        force_n: [f64; 3],
        torque_nm: [f64; 3],
    },
    Range {
        distance_m: f64,
    },
    Battery {
        voltage_v: f64,
        current_a: f64,
        level_pct: f64,
        temperature_c: f64,
        charging: bool,
        healthy: bool,
        capacity_ah: f64,
    },
}
#[derive(Clone, Serialize, Deserialize)]
pub struct PointField {
    pub name: String,
    pub offset: u32,
    pub datatype: u8,
    pub count: u32,
}

pub enum SensorOutput {
    Occupancy(
        ConfiguredObservable<crate::robot_occupancy::RobotOccupancy>,
        BufferProductCapture<crate::robot_occupancy::RobotOccupancy>,
    ),
    Pose(
        ConfiguredObservable<auki_odometry::PoseUpdate>,
        BufferProductCapture<auki_odometry::PoseUpdate>,
    ),
    Camera(
        ConfiguredObservable<VideoFrame>,
        BufferProductCapture<VideoFrame>,
    ),
    Measurement(
        ConfiguredObservable<SensorReading>,
        BufferProductCapture<SensorReading>,
    ),
}
pub struct Sensor {
    pub portal_detector: Option<crate::portal_camera_detector::PortalCameraDetector>,
    pub spec: SensorSpec,
    pub output: SensorOutput,
    pub last: Option<u64>,
    pub count: u64,
}
impl Sensor {
    pub fn mount(
        runtime: &ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        mut spec: SensorSpec,
    ) -> Result<Self> {
        // Pictures go out in the binary camera codec, never as JSON, whatever
        // schema the local sensor source declared for its input.
        if let PayloadContract::Camera(c) = &mut spec.payload && c.schema == crate::camera_frame::LEGACY_SCHEMA {
            c.schema = crate::camera_frame::SCHEMA.into();
        }
        if spec.id.is_empty()
            || spec
                .frame_definition
                .as_ref()
                .is_some_and(|f| f.frame_id.is_empty())
        {
            return Err("invalid sensor identity/frame".into());
        }
        let spatial = match &spec.payload {
            PayloadContract::Camera(_) => true,
            PayloadContract::Structured(c) => matches!(
                c.modality.as_str(),
                "point_cloud" | "depth" | "imu" | "wrench" | "pose" | "robot_occupancy"
            ),
            _ => false,
        };
        if spatial && spec.frame_definition.is_none() {
            return Err("spatial sensor requires an explicit source frame".into());
        }
        let (datatype, schema) = match &spec.payload {
            PayloadContract::Camera(c) if c.encoding == "jpeg" && c.width > 0 && c.height > 0 => {
                (c.datatype.clone(), c.schema.clone())
            }
            PayloadContract::Structured(c) => (c.datatype.clone(), c.schema.clone()),
            _ => return Err("unsupported sensor payload contract".into()),
        };
        let component = runtime
            .component(ComponentSpec::new(&spec.id).observable(ObservableContract {
                name: "samples".into(),
                datatype,
                schema,
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }))
            .map_err(err)?;
        let mut configured = ConfiguredObservableSpec::new(
            "samples",
            Uuid::new_v4().to_string(),
            spec.clock.clone(),
            spec.payload.clone(),
        );
        if let Some(frame) = &spec.frame_definition {
            configured = configured.in_registered_frame(frame.clone());
        }
        let product_id = format!("{}-history", spec.id);
        let metadata = spec.metadata.clone();
        let metadata_fn = move |observation: &Observation<SensorReading>| {
            Ok(Some(CatalogProductMetadata {
                schema: "galbot.sensor-metadata/v1".into(),
                source_sequence: observation.sequence,
                value: metadata.clone(),
            }))
        };
        let output = match &spec.payload {
            PayloadContract::Structured(c)
                if c.datatype == crate::robot_occupancy::RobotOccupancy::DATATYPE =>
            {
                let observable = component
                    .configured_observable::<crate::robot_occupancy::RobotOccupancy>(configured)
                    .map_err(err)?;
                component.expose().map_err(err)?;
                let metadata = spec.metadata.clone();
                let capture = runtime
                    .capture_buffer_with_metadata(
                        &product_id,
                        &observable,
                        BufferLimits {
                            max_entries: Some(2),
                            max_bytes: Some(4 * 1024 * 1024),
                            target_duration: Some(Duration::from_secs(2)),
                        },
                        |v: &crate::robot_occupancy::RobotOccupancy| {
                            serde_json::to_vec(v).map_or(0, |b| b.len())
                        },
                        move |o| {
                            Ok(Some(CatalogProductMetadata {
                                schema: "spatial.robot-occupancy-model/v1".into(),
                                source_sequence: o.sequence,
                                value: metadata.clone(),
                            }))
                        },
                    )
                    .map_err(err)?;
                endpoint.export_product(&capture.product()).map_err(err)?;
                SensorOutput::Occupancy(observable, capture)
            }
            PayloadContract::Camera(_) => {
                let observable = component
                    .configured_observable::<VideoFrame>(configured)
                    .map_err(err)?;
                component.expose().map_err(err)?;
                let metadata = spec.metadata.clone();
                let capture = runtime
                    .capture_buffer_with_metadata(
                        &product_id,
                        &observable,
                        history_limits(),
                        |v: &VideoFrame| v.bytes.len(),
                        move |observation| {
                            Ok(Some(CatalogProductMetadata {
                                schema: "galbot.sensor-metadata/v1".into(),
                                source_sequence: observation.sequence,
                                value: metadata.clone(),
                            }))
                        },
                    )
                    .map_err(err)?;
                endpoint.export_product_with_encoder(&capture.product(), crate::camera_frame::encoder()).map_err(err)?;
                SensorOutput::Camera(observable, capture)
            }
            PayloadContract::Structured(c) if c.datatype == auki_odometry::PoseUpdate::DATATYPE => {
                let contract: auki_odometry::PoseContract =
                    serde_json::from_value(spec.metadata["pose_contract"].clone()).map_err(err)?;
                contract.validate().map_err(err)?;
                if contract.clock != spec.clock
                    || spec.frame_definition.as_ref() != Some(&contract.to_frame)
                {
                    return Err("pose output differs from declared contract".into());
                }
                let observable = component
                    .configured_observable::<auki_odometry::PoseUpdate>(configured)
                    .map_err(err)?;
                component.expose().map_err(err)?;
                let metadata = serde_json::to_value(&contract).map_err(err)?;
                let capture = runtime
                    .capture_buffer_with_metadata(
                        &product_id,
                        &observable,
                        history_limits(),
                        |v: &auki_odometry::PoseUpdate| {
                            serde_json::to_vec(v).map_or(0, |b| b.len())
                        },
                        move |o| {
                            Ok(Some(CatalogProductMetadata {
                                schema: "auki.odometry-product/v1".into(),
                                source_sequence: o.sequence,
                                value: metadata.clone(),
                            }))
                        },
                    )
                    .map_err(err)?;
                endpoint.export_product(&capture.product()).map_err(err)?;
                SensorOutput::Pose(observable, capture)
            }
            _ => {
                let observable = component
                    .configured_observable::<SensorReading>(configured)
                    .map_err(err)?;
                component.expose().map_err(err)?;
                let capture = runtime
                    .capture_buffer_with_metadata(
                        &product_id,
                        &observable,
                        history_limits(),
                        |v: &SensorReading| serde_json::to_vec(v).map_or(0, |b| b.len()),
                        metadata_fn,
                    )
                    .map_err(err)?;
                endpoint.export_product(&capture.product()).map_err(err)?;
                SensorOutput::Measurement(observable, capture)
            }
        };
        Ok(Self {
            portal_detector: None,
            spec,
            output,
            last: None,
            count: 0,
        })
    }
    pub fn publish(&mut self, timestamp: u64, data: &str, bytes: Vec<u8>) -> Result<()> {
        if timestamp == 0 || self.last.is_some_and(|last| timestamp <= last) {
            return Err("sensor timestamp must strictly advance on its declared clock".into());
        }
        if data.len() > 24 * 1024 * 1024 || bytes.len() > 4 * 1024 * 1024 {
            return Err("sensor sample exceeds bounded transport payload size".into());
        }
        match &self.output {
            SensorOutput::Occupancy(output, _) => {
                let value: crate::robot_occupancy::RobotOccupancy =
                    serde_json::from_str(data).map_err(err)?;
                value.validate().map_err(err)?;
                if self.spec.metadata["model_sha256"] != value.model_sha256
                    || self.spec.metadata["session_id"] != value.session_id
                {
                    return Err("Robot model changed".into());
                }
                output.publish(timestamp, Arc::new(value)).map_err(err)?;
            }
            SensorOutput::Pose(output, _) => {
                let update: auki_odometry::PoseUpdate = serde_json::from_str(data).map_err(err)?;
                update.validate().map_err(err)?;
                if serde_json::to_value(&update.contract).map_err(err)?
                    != self.spec.metadata["pose_contract"]
                {
                    return Err("sensor pose frame/clock/session contract changed".into());
                }
                output.publish(timestamp, Arc::new(update)).map_err(err)?;
            }
            SensorOutput::Camera(output, _) => {
                let PayloadContract::Camera(c) = &self.spec.payload else {
                    unreachable!()
                };
                let reader = image::ImageReader::with_format(
                    std::io::Cursor::new(&bytes),
                    image::ImageFormat::Jpeg,
                );
                if reader.into_dimensions().map_err(err)? != (c.width, c.height) {
                    return Err("JPEG dimensions differ from camera contract".into());
                }
                let (observation, _) = output
                    .publish(
                        timestamp,
                        Arc::new(VideoFrame {
                            width: c.width,
                            height: c.height,
                            encoding: "jpeg".into(),
                            bytes: bytes.into(),
                        }),
                    )
                    .map_err(err)?;
                if let Some(detector) = &self.portal_detector {
                    detector.submit(observation);
                }
            }
            SensorOutput::Measurement(output, _) => {
                let reading: SensorReading = serde_json::from_str(data).map_err(err)?;
                if reading.sensor_id != self.spec.id {
                    return Err("sensor identity mismatch".into());
                }
                let kind = match &reading.measurement {
                    Measurement::Joint { .. } => "joint",
                    Measurement::PointCloud { .. } => "point_cloud",
                    Measurement::Depth { .. } => "depth",
                    Measurement::Imu { .. } => "imu",
                    Measurement::Wrench { .. } => "wrench",
                    Measurement::Range { .. } => "range",
                    Measurement::Battery { .. } => "battery",
                };
                let PayloadContract::Structured(contract) = &self.spec.payload else {
                    unreachable!()
                };
                if contract.modality != kind {
                    return Err("sensor measurement kind differs from contract".into());
                }
                if matches!(kind, "point_cloud" | "depth" | "imu" | "wrench" | "pose")
                    && self.spec.frame_definition.is_none()
                {
                    return Err("spatial sensor requires an explicit source frame".into());
                }
                output.publish(timestamp, Arc::new(reading)).map_err(err)?;
            }
        }
        self.last = Some(timestamp);
        self.count += 1;
        Ok(())
    }
    pub fn status(&self) -> serde_json::Value {
        let product = match &self.output {
            SensorOutput::Occupancy(_, capture) => capture.product().reference(),
            SensorOutput::Pose(_, capture) => capture.product().reference(),
            SensorOutput::Camera(_, capture) => capture.product().reference(),
            SensorOutput::Measurement(_, capture) => capture.product().reference(),
        };
        serde_json::json!({"product": product, "published": self.count, "last_timestamp_ns": self.last, "clock": self.spec.clock,
            "portal_detector": self.portal_detector.as_ref().map(|d| d.status())})
    }
    pub fn end(&self) {
        if let Some(timestamp) = self.last {
            let reason = ObservationEndReason::Reconfigured { replacement: None };
            match &self.output {
                SensorOutput::Occupancy(output, _) => {
                    let _ = output.end(timestamp, reason);
                }
                SensorOutput::Pose(output, _) => {
                    let _ = output.end(timestamp, reason);
                }
                SensorOutput::Camera(output, _) => {
                    let _ = output.end(timestamp, reason);
                }
                SensorOutput::Measurement(output, _) => {
                    let _ = output.end(timestamp, reason);
                }
            }
        }
    }
}
