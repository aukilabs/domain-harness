//! An idealized, instantaneous 3D range scan in a sensor-local OpenGL frame.
//! Null ranges mean no return (including hits inside minimum range), not occupied.
use anyhow::{Result, ensure};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub const PRODUCT: &str = "simulator-lidar";
pub const SCHEMA: &str = "spatial.simulated-lidar-scan/v1";
pub const COLUMNS: usize = 121;
pub const ROWS: usize = 16;
pub const MAX_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scan {
    pub timestamp_ns: u64,
    /// Row-major: elevation -15..+15 degrees, azimuth -60..+60 degrees.
    pub ranges_m: Vec<Option<f32>>,
    /// Simulator ground truth, not a calibrated transform into Home's frame.
    pub world_position: [f32; 3],
    pub world_rotation_xyzw: [f32; 4],
}
impl ContractType for Scan {
    const DATATYPE: &'static str = SCHEMA;
}

/// The scan on the wire: never JSON. "SLD1", capture time (u64), world
/// position (3 × f32) and rotation (4 × f32), then ROWS × COLUMNS ranges as
/// f32, NaN for no return. Little endian.
pub const ENCODING: &str = "spatial-harness.simulated-lidar-packed/v1";
const HEADER: usize = 4 + 8 + 7 * 4;
pub fn pack(scan: &Scan) -> Result<Vec<u8>> {
    scan.validate()?;
    let mut bytes = Vec::with_capacity(HEADER + scan.ranges_m.len() * 4);
    bytes.extend_from_slice(b"SLD1");
    bytes.extend_from_slice(&scan.timestamp_ns.to_le_bytes());
    for v in scan.world_position.iter().chain(&scan.world_rotation_xyzw) {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    for range in &scan.ranges_m {
        bytes.extend_from_slice(&range.unwrap_or(f32::NAN).to_le_bytes());
    }
    Ok(bytes)
}
pub fn unpack(bytes: &[u8]) -> Result<Scan> {
    ensure!(bytes.len() == HEADER + COLUMNS * ROWS * 4 && &bytes[..4] == b"SLD1", "Invalid LiDAR scan packet");
    let f = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let scan = Scan {
        timestamp_ns: u64::from_le_bytes(bytes[4..12].try_into().unwrap()),
        world_position: std::array::from_fn(|i| f(12 + i * 4)),
        world_rotation_xyzw: std::array::from_fn(|i| f(24 + i * 4)),
        ranges_m: (0..COLUMNS * ROWS).map(|i| Some(f(HEADER + i * 4)).filter(|r| !r.is_nan())).collect(),
    };
    scan.validate()?;
    Ok(scan)
}
pub fn encoder() -> auki_component_protocol::ObservationEncoder<Scan> {
    auki_component_protocol::ObservationEncoder::new(ENCODING, |s| pack(s).map_err(|e| e.to_string()))
        .expect("static LiDAR codec")
}
pub fn decoder() -> Result<auki_component_protocol::ObservationDecoder<Scan>> {
    Ok(auki_component_protocol::ObservationDecoder::new(ENCODING, |b| unpack(b).map_err(|e| e.to_string()))?)
}
impl Scan {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.timestamp_ns > 0 && self.ranges_m.len() == COLUMNS * ROWS,
            "Invalid LiDAR timestamp/beam count"
        );
        ensure!(
            self.ranges_m
                .iter()
                .flatten()
                .all(|r| r.is_finite() && (0.1..=10.0).contains(r)),
            "Invalid LiDAR range"
        );
        ensure!(
            self.world_position
                .iter()
                .all(|v| v.is_finite() && v.abs() < 10000.0),
            "Invalid LiDAR position"
        );
        ensure!(
            self.world_rotation_xyzw.iter().all(|v| v.is_finite())
                && (self.world_rotation_xyzw.iter().map(|v| v * v).sum::<f32>() - 1.0).abs()
                    < 0.001,
            "Invalid LiDAR orientation"
        );
        Ok(())
    }
    pub fn local_hits(&self) -> Vec<[f32; 3]> {
        self.ranges_m
            .iter()
            .enumerate()
            .filter_map(|(i, range)| {
                let r = (*range)?;
                let yaw = (-60.0 + (i % COLUMNS) as f32).to_radians();
                let elevation = (-15.0 + (i / COLUMNS) as f32 * 2.0).to_radians();
                Some([
                    r * yaw.sin() * elevation.cos(),
                    r * elevation.sin(),
                    -r * yaw.cos() * elevation.cos(),
                ])
            })
            .collect()
    }
}
pub struct Lidar {
    local_consumers: Arc<AtomicUsize>,
    output: ConfiguredObservable<Scan>,
    capture: BufferProductCapture<Scan>,
    _component: Component,
    last_timestamp: u64,
}
impl Lidar {
    // SDK buffer bindings are local consumers, not protocol subscriptions.
    // Keep their demand lease tied to the actual binding's lifetime.
    pub fn bind_input(
        &self,
        component: &Component,
        input: &InputPort<Observation<Scan>>,
    ) -> Result<LocalScanInput> {
        let binding = component.configured_buffer_input(
            "lidar",
            &self.product(),
            CursorStart::Current,
            input,
        )?;
        self.local_consumers.fetch_add(1, Ordering::SeqCst);
        Ok(LocalScanInput {
            _binding: binding,
            consumers: self.local_consumers.clone(),
        })
    }
    pub fn product(&self) -> RetainedProduct<Scan> {
        self.capture.product()
    }
    pub fn mount(runtime: &ComponentRuntime, endpoint: &ComponentProtocolEndpoint) -> Result<Self> {
        let component = runtime.component(
            ComponentSpec::new("simulator-forward-lidar").observable(ObservableContract {
                name: "scans".into(),
                datatype: SCHEMA.into(),
                schema: SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }),
        )?;
        let session = uuid::Uuid::new_v4().to_string();
        let clock = crate::model::clock_entry(runtime.peer_id(), &session);
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "scans",
                session.clone(),
                crate::model::clock_ref(&clock),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "range_scan".into(),
                    datatype: SCHEMA.into(),
                    schema: SCHEMA.into(),
                    observes: "forward_3d_lidar".into(),
                    unit: Some("meters".into()),
                }),
            )
            .in_registered_frame(auki_registry::FrameRegistryEntry::in_convention(
                runtime.peer_id(),
                format!("simulator:{session}:lidar"),
                auki_registry::CoordinateConvention::OpenGl,
            )),
        )?;
        component.expose()?;
        let capture = runtime.capture_buffer_with_metadata(PRODUCT, &output,
            BufferLimits { max_entries:Some(1), max_bytes:Some(MAX_BYTES), target_duration:None },
            |s: &Scan| HEADER + s.ranges_m.len() * 4,
            |o| Ok(Some(CatalogProductMetadata {
                schema:"spatial.simulated-lidar-model/v1".into(), source_sequence:o.sequence,
                value:serde_json::json!({"horizontal_fov_deg":120,"vertical_fov_deg":30,"columns":COLUMNS,"rows":ROWS,"min_range_m":0.1,"max_range_m":10,"rate_hz":10,"mount_translation_m":[0,0.65,-0.28],"axes":"x right, y up, -z forward","pose":"simulator ground truth, unaligned","sampling":"instantaneous first surface hit; no noise or rolling scan"}),
            }))
        )?;
        endpoint.export_product_with_encoder(&capture.product(), encoder())?;
        Ok(Self {
            local_consumers: Arc::new(AtomicUsize::new(0)),
            output,
            capture,
            _component: component,
            last_timestamp: 0,
        })
    }
    pub fn demanded(&self, endpoint: &ComponentProtocolEndpoint) -> bool {
        endpoint.product_active_subscriptions(PRODUCT) > 0
            || self.local_consumers.load(Ordering::SeqCst) > 0
    }
    pub fn publish(&mut self, scan: Scan) -> Result<()> {
        scan.validate()?;
        ensure!(
            scan.timestamp_ns > self.last_timestamp,
            "LiDAR timestamp did not advance"
        );
        let timestamp = scan.timestamp_ns;
        self.output.publish(timestamp, Arc::new(scan))?;
        self.last_timestamp = timestamp;
        ensure!(self.capture.errors().is_empty(), "LiDAR retention failed");
        Ok(())
    }
    pub fn close(self) {
        self.capture.cancel();
    }
}
pub struct LocalScanInput {
    _binding: ConfiguredBufferInput<Scan>,
    consumers: Arc<AtomicUsize>,
}
impl Drop for LocalScanInput {
    fn drop(&mut self) {
        self.consumers.fetch_sub(1, Ordering::SeqCst);
    }
}
