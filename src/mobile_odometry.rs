//! Phone pose as a declared SDK Product, alongside the pose pushed to Home.
//! The push keeps presence working; this makes the same pose discoverable in
//! the phone's own Catalog with explicit frames and clock.
use anyhow::{Result, ensure};
use auki_components::*;
use auki_odometry::{METADATA_SCHEMA, POSE_SCHEMA, Pose, PoseContract, PoseUpdate, Tracking};
use auki_registry::FrameRegistryEntry;
use std::sync::Arc;

pub const COMPONENT: &str = "phone-pose";
const SLOT: &str = "poses";

pub fn product_id(session: &str) -> String {
    format!("phone-pose:{session}")
}

/// The camera frame and capture clock are the ones phone depth declares, so
/// a reader can relate a depth sample to the pose of the same AR session.
/// The destination is Home's scene frame, owned by Home.
pub fn contract(owner: &str, home_peer: &str, sample: &crate::mobile_pose::Sample) -> PoseContract {
    let mut clock = crate::model::clock_entry(owner, &sample.session);
    clock.clock_id = "arkit-capture".into();
    PoseContract {
        from_frame: FrameRegistryEntry::opengl(owner, &sample.pose.from_frame_id),
        to_frame: FrameRegistryEntry::opengl(home_peer, &sample.pose.to_frame_id),
        clock: crate::model::clock_ref(&clock),
        session_id: sample.session.clone(),
    }
}

fn spec(contract: &PoseContract) -> ConfiguredObservableSpec {
    ConfiguredObservableSpec::new(
        SLOT,
        format!("poses:{}:{}", contract.session_id, uuid::Uuid::new_v4()),
        contract.clock.clone(),
        PayloadContract::Structured(StructuredPayloadContract {
            modality: "pose".into(),
            datatype: PoseUpdate::DATATYPE.into(),
            schema: POSE_SCHEMA.into(),
            observes: "phone_camera_pose_in_home_scene".into(),
            unit: None,
        }),
    )
    .in_registered_frame(contract.to_frame.clone())
}

fn capture(
    runtime: &ComponentRuntime,
    output: &ConfiguredObservable<PoseUpdate>,
    contract: &PoseContract,
) -> Result<BufferProductCapture<PoseUpdate>> {
    let expected = contract.clone();
    Ok(runtime.capture_buffer_with_metadata(
        product_id(&contract.session_id),
        output,
        BufferLimits {
            max_entries: Some(300),
            max_bytes: Some(1024 * 1024),
            target_duration: None,
        },
        |update: &PoseUpdate| serde_json::to_vec(update).map_or(usize::MAX, |v| v.len()),
        move |observation| {
            Ok(Some(CatalogProductMetadata {
                schema: METADATA_SCHEMA.into(),
                source_sequence: observation.sequence,
                value: serde_json::to_value(&expected).map_err(|e| e.to_string())?,
            }))
        },
    )?)
}

pub struct Publisher {
    contract: PoseContract,
    output: ConfiguredObservable<PoseUpdate>,
    capture: Option<BufferProductCapture<PoseUpdate>>,
    endpoint: Arc<auki_component_protocol::ComponentProtocolEndpoint>,
    last_timestamp: u64,
    lost: bool,
    component: Component,
    runtime: ComponentRuntime,
}
impl Publisher {
    pub fn new(
        products: &crate::mobile_products::MobileProducts,
        home_peer: &str,
        sample: &crate::mobile_pose::Sample,
    ) -> Result<Self> {
        uuid::Uuid::parse_str(&sample.session)?;
        let runtime = products.runtime.clone();
        let contract = contract(runtime.peer_id(), home_peer, sample);
        contract.validate()?;
        let component =
            runtime.component(ComponentSpec::new(COMPONENT).observable(ObservableContract {
                name: SLOT.into(),
                datatype: PoseUpdate::DATATYPE.into(),
                schema: POSE_SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }))?;
        let output = component.configured_observable(spec(&contract))?;
        component.expose()?;
        let capture = capture(&runtime, &output, &contract)?;
        let endpoint = products.endpoint.clone();
        endpoint.export_product(&capture.product())?;
        Ok(Self {
            contract,
            output,
            capture: Some(capture),
            endpoint,
            last_timestamp: 0,
            lost: false,
            component,
            runtime,
        })
    }
    pub fn contract(&self) -> &PoseContract {
        &self.contract
    }
    /// A sample that does not advance the capture clock is skipped, not an error.
    pub fn publish(&mut self, home_peer: &str, sample: &crate::mobile_pose::Sample) -> Result<()> {
        let next = contract(self.runtime.peer_id(), home_peer, sample);
        if next != self.contract {
            self.reconfigure(next, sample.timestamp_ns)?;
        }
        if sample.timestamp_ns <= self.last_timestamp {
            return Ok(());
        }
        let [w, x, y, z] = sample.pose.rotation_wxyz;
        let norm = (w * w + x * x + y * y + z * z).sqrt();
        ensure!(norm.is_finite() && norm > 0.5, "Invalid phone pose");
        let pose = Pose {
            translation: sample.pose.translation,
            rotation_xyzw: [x / norm, y / norm, z / norm, w / norm],
        };
        self.send(sample.timestamp_ns, Tracking::Valid { pose })?;
        self.lost = false;
        Ok(())
    }
    /// The phone stopped reporting a pose. Said once, on the same capture clock.
    pub fn lost(&mut self) -> Result<()> {
        if self.lost || self.last_timestamp == 0 {
            return Ok(());
        }
        self.send(self.last_timestamp + 1, Tracking::Lost)?;
        self.lost = true;
        Ok(())
    }
    fn send(&mut self, timestamp_ns: u64, tracking: Tracking) -> Result<()> {
        let update = PoseUpdate {
            contract: self.contract.clone(),
            tracking,
        };
        update.validate()?;
        self.output.publish(timestamp_ns, Arc::new(update))?;
        self.last_timestamp = timestamp_ns;
        ensure!(
            self.capture.as_ref().is_some_and(|c| c.errors().is_empty()),
            "Pose Product capture failed"
        );
        Ok(())
    }
    /// A new AR session or scene is a new Output and Product on the same Component.
    fn reconfigure(&mut self, contract: PoseContract, timestamp_ns: u64) -> Result<()> {
        contract.validate()?;
        let replacement = self
            .component
            .replace_configured_observable(&self.output, spec(&contract), timestamp_ns)?
            .replacement;
        self.withdraw()?;
        self.output = replacement;
        self.contract = contract;
        self.last_timestamp = 0;
        self.lost = false;
        let capture = capture(&self.runtime, &self.output, &self.contract)?;
        self.endpoint.export_product(&capture.product())?;
        self.capture = Some(capture);
        Ok(())
    }
    fn withdraw(&mut self) -> Result<()> {
        if let Some(capture) = self.capture.take() {
            self.endpoint
                .unexport_product(&capture.product().manifest.product_id);
            capture.delete()?;
        }
        Ok(())
    }
    pub fn close(mut self) -> Result<()> {
        self.withdraw()
    }
}
