//! Compact native AR pose Product; interpretation lives in Registry v3.
use anyhow::{Result, ensure};
use auki_components::*;
use auki_odometry::{Pose, PoseContract, Tracking};
use crate::compact_pose::{Update as PoseUpdate, SCHEMA as POSE_SCHEMA, METADATA as METADATA_SCHEMA};
use auki_protocols::registry::v3::{RegistryEntryEnvelope,RegistryKind};
use auki_registry::FrameRegistryEntry;
use std::sync::Arc;

pub const COMPONENT: &str = "phone-pose";
const SLOT: &str = "poses";

pub fn product_id(session: &str) -> String {
    format!("phone-pose:{session}")
}

/// The camera frame and capture clock are the ones phone depth declares, so
/// a reader can relate a depth sample to the pose of the same AR session.
/// Both endpoints belong to this phone, independent of Home alignment.
pub fn contract(owner: &str, _home_peer: &str, sample: &crate::mobile_pose::Sample, clock: &auki_components::clock::ClockRegistryEntry) -> PoseContract {
    PoseContract {
        from_frame: FrameRegistryEntry::opengl(owner, &sample.pose.from_frame_id),
        to_frame: FrameRegistryEntry::opengl(owner, &sample.pose.to_frame_id),
        clock: crate::model::clock_ref(&clock),
        session_id: sample.session.clone(),
    }
}


fn compact(c:&PoseContract)->crate::compact_pose::Contract {
    crate::compact_pose::Contract{from_frame:(&c.from_frame).into(),to_frame:(&c.to_frame).into(),clock:c.clock.clone(),session_id:c.session_id.clone(),lifetime:Default::default()}
}
fn validate_native(c:&PoseContract)->Result<()> {
    uuid::Uuid::parse_str(&c.session_id)?;
    c.validate()?;
    ensure!(c.from_frame.frame_id==format!("phone-camera:{}",c.session_id)
        && c.to_frame.frame_id==format!("phone-ar:{}",c.session_id),"Phone pose must be in native AR session space");
    Ok(())
}
fn register(entries:&Arc<std::sync::Mutex<Vec<RegistryEntryEnvelope>>>,c:&PoseContract,clock:&auki_components::clock::ClockRegistryEntry)->Result<()> {
    ensure!(clock.hash()==c.clock.hash,"Clock definition mismatch");
    let mut entries=entries.lock().map_err(|_|anyhow::anyhow!("Registry unavailable"))?;
    for entry in [
        RegistryEntryEnvelope{kind:RegistryKind::Clock,id:clock.clock_id.clone(),hash:clock.hash(),canonical_json:String::from_utf8(clock.canonical_bytes())?},
        RegistryEntryEnvelope{kind:RegistryKind::Frame,id:c.from_frame.frame_id.clone(),hash:c.from_frame.hash(),canonical_json:String::from_utf8(c.from_frame.canonical_bytes())?},
        RegistryEntryEnvelope{kind:RegistryKind::Frame,id:c.to_frame.frame_id.clone(),hash:c.to_frame.hash(),canonical_json:String::from_utf8(c.to_frame.canonical_bytes())?},
    ] {if !entries.iter().any(|e|e.kind==entry.kind&&e.id==entry.id&&e.hash==entry.hash){entries.push(entry);}}
    Ok(())
}

fn spec(contract: &PoseContract) -> ConfiguredObservableSpec {
    ConfiguredObservableSpec::new(
        SLOT,
        compact(contract).output_id(),
        contract.clock.clone(),
        PayloadContract::Structured(StructuredPayloadContract {
            modality: "pose".into(),
            datatype: PoseUpdate::DATATYPE.into(),
            schema: POSE_SCHEMA.into(),
            observes: "phone_camera_pose_in_ar_session".into(),
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
    let expected = compact(contract);
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
    clock: auki_components::clock::ClockRegistryEntry,
    session_clock: auki_time::SessionClock,
    contract: PoseContract,
    output: ConfiguredObservable<PoseUpdate>,
    capture: Option<BufferProductCapture<PoseUpdate>>,
    endpoint: Arc<auki_component_protocol::ComponentProtocolEndpoint>,
    last_timestamp: u64,
    lost: bool,
    component: Component,
    runtime: ComponentRuntime,
    definitions: Arc<std::sync::Mutex<Vec<RegistryEntryEnvelope>>>,
}
impl Publisher {
    pub fn new(
        products: &crate::mobile_products::MobileProducts,
        home_peer: &str,
        sample: &crate::mobile_pose::Sample,
    ) -> Result<Self> {
        uuid::Uuid::parse_str(&sample.session)?;
        let runtime = products.runtime.clone();
        let clock = products.timing.clock.registry_entry().clone();
        let contract = contract(runtime.peer_id(), home_peer, sample, &clock);
        validate_native(&contract)?;
        register(&products.definitions,&contract,&clock)?;
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
            clock,
            session_clock: products.timing.clock.clone(),
            contract,
            output,
            capture: Some(capture),
            endpoint,
            last_timestamp: 0,
            lost: false,
            component,
            runtime,
            definitions: products.definitions.clone(),
        })
    }
    pub fn contract(&self) -> &PoseContract {
        &self.contract
    }
    /// A sample that does not advance the capture clock is skipped, not an error.
    pub fn publish(&mut self, home_peer: &str, sample: &crate::mobile_pose::Sample) -> Result<()> {
        let next = contract(self.runtime.peer_id(), home_peer, sample, &self.clock);
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
        let at = self.session_clock.now_ns();
        ensure!(at > self.last_timestamp, "Tracking event precedes its last SDK capture time");
        self.send(at, Tracking::Lost)?;
        self.lost = true;
        Ok(())
    }
    fn send(&mut self, timestamp_ns: u64, tracking: Tracking) -> Result<()> {
        let update = PoseUpdate { tracking };
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
        validate_native(&contract)?;
        register(&self.definitions,&contract,&self.clock)?;
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

#[cfg(test)]
mod session_clock_tests {
    use super::*;
    #[test]
    fn delayed_native_pose_roundtrips_through_sdk_buffer_on_declared_clock() -> Result<()> {
        let timing = crate::mobile_products::CaptureTiming::new("phone");
        timing.observe(0, 100_000_000, 50_100_000_010, 100_000_020)?;
        timing.observe(0, 200_000_000, 50_200_000_010, 200_000_020)?;
        let session = uuid::Uuid::new_v4().to_string();
        let native = crate::mobile_pose::Sample {
            session: session.clone(), timestamp_ns: 50_150_000_000, camera_configuration_id: None,
            pose: auki_scenegraph::RigidTransform { from_frame_id: format!("phone-camera:{session}"),
                to_frame_id: format!("phone-ar:{session}"), translation: [1.,2.,3.], rotation_wxyz: [1.,0.,0.,0.] },
        };
        let sample = timing.pose(native.clone())?;
        assert_eq!(sample.timestamp_ns, 150_000_000);
        assert_eq!(native.timestamp_ns, 50_150_000_000, "Native app evidence is not rewritten");
        let clock = timing.clock.registry_entry();
        let contract = contract("phone", "home", &sample, &clock);
        let runtime = ComponentRuntime::new("phone");
        let component = runtime.component(ComponentSpec::new(COMPONENT).observable(ObservableContract {
            name: SLOT.into(), datatype: PoseUpdate::DATATYPE.into(), schema: POSE_SCHEMA.into(),
            access: vec![ObservationAccess::FollowNew], exposure: Exposure::Cluster,
        }))?;
        let output = component.configured_observable(spec(&contract))?;
        component.expose()?;
        let retained = capture(&runtime, &output, &contract)?;
        let definitions = Arc::new(std::sync::Mutex::new(Vec::new()));
        register(&definitions, &contract, &clock)?;
        output.publish(sample.timestamp_ns, Arc::new(PoseUpdate { tracking: Tracking::Valid {
            pose: Pose { translation: sample.pose.translation, rotation_xyzw: [0.,0.,0.,1.] },
        }}))?;
        let observed = retained.product().latest_existing()?.unwrap();
        assert_eq!(observed.timestamp_ns, 150_000_000);
        assert_eq!(output.manifest().clock, crate::model::clock_ref(&clock));
        compact(&contract).validate_output(&output.manifest())?;
        let utc = crate::capture_time::Contract::local(clock.clone());
        assert_eq!(crate::model::clock_ref(&utc.from_clock), output.manifest().clock,
            "The declared full clock reference selects the exact UTC-transform Product");
        assert!(definitions.lock().unwrap().iter().any(|d| d.kind == RegistryKind::Clock && d.hash == clock.hash()));
        retained.delete()?;
        Ok(())
    }
}
