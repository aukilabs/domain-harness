use anyhow::{Result, ensure};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;
use auki_odometry::{Pose, PoseContract, Tracking};
use auki_protocols::registry::{RegistryEndpoint, v3::*};
use auki_registry::{
    ClockBody, ClockMeta, ClockRegistryEntry, CoordinateConvention, FrameRegistryEntry, Scope,
};
use auki_sdk::AukiPeer;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Sample {
    pub timestamp_ns: u64,
    pub pose: Option<Pose>,
    pub source_id: Option<u32>,
}

pub struct Publisher {
    pub runtime: ComponentRuntime,
    pub contract: PoseContract,
    pub definitions: std::sync::Arc<std::sync::Mutex<Vec<(RegistryKind, String, String, String)>>>,
    _component: Component,
    output: ConfiguredObservable<crate::compact_pose::Update>,
    _capture: BufferProductCapture<crate::compact_pose::Update>,
    pub endpoint: ComponentProtocolEndpoint,
    pub registry: RegistryEndpoint,
    last: u64,
    source: Option<u32>,
}

impl Publisher {
    pub fn product(&self) -> RetainedProduct<crate::compact_pose::Update> {
        self._capture.product()
    }
    pub fn compact(&self) -> crate::compact_pose::Contract {
        crate::compact_pose::Contract {
            from_frame: (&self.contract.from_frame).into(),
            to_frame: (&self.contract.to_frame).into(),
            clock: self.contract.clock.clone(),
            session_id: self.contract.session_id.clone(),
            lifetime: crate::compact_pose::Lifetime::Sampled,
        }
    }
    pub fn mount(peer: &AukiPeer) -> Result<Self> {
        let owner = peer.peer_id().to_string();
        let session = uuid::Uuid::new_v4().to_string();
        // BBOS timestamps are device wall-clock capture ns, not host receipt time.
        let clock = ClockRegistryEntry {
            peer_id: owner.clone(),
            session_id: session.clone(),
            clock_id: "bbos-capture".into(),
            body: ClockBody::UtcClock(ClockMeta {
                unit: "nanoseconds".into(),
                monotonic: false,
                epoch: Some("1970-01-01T00:00:00Z".into()),
                scope: Scope::DeviceLocal,
            }),
        };
        let contract = PoseContract {
            from_frame: FrameRegistryEntry::in_convention(
                &owner,
                "bbos-base",
                CoordinateConvention::ZUpRightForward,
            ),
            to_frame: FrameRegistryEntry::in_convention(
                &owner,
                format!("bbos-world-{session}"),
                CoordinateConvention::ZUpRightForward,
            ),
            clock: ClockReference {
                peer_id: owner.clone(),
                id: clock.clock_id.clone(),
                hash: clock.hash(),
            },
            session_id: session,
        };
        let entries = vec![
            (
                RegistryKind::Clock,
                clock.clock_id.clone(),
                clock.hash(),
                String::from_utf8(clock.canonical_bytes())?,
            ),
            (
                RegistryKind::Frame,
                contract.from_frame.frame_id.clone(),
                contract.from_frame.hash(),
                String::from_utf8(contract.from_frame.canonical_bytes())?,
            ),
            (
                RegistryKind::Frame,
                contract.to_frame.frame_id.clone(),
                contract.to_frame.hash(),
                String::from_utf8(contract.to_frame.canonical_bytes())?,
            ),
        ];
        let definitions = std::sync::Arc::new(std::sync::Mutex::new(entries));
        let served = definitions.clone();
        let registry = RegistryEndpoint::mount(
            peer.protocols(),
            move |_: &auki_sdk::AuthenticatedPeer, request: &RegistryRequest| {
                let entries = served.lock().unwrap();
                match request {
                    RegistryRequest::List { kind } => RegistryResponse::List {
                        entries: entries
                            .iter()
                            .filter(|e| e.0 == *kind)
                            .map(|e| RegistryListEntry {
                                id: e.1.clone(),
                                hash: e.2.clone(),
                            })
                            .collect(),
                    },
                    RegistryRequest::Get { kind, id, hash } => RegistryResponse::Get {
                        entry: entries
                            .iter()
                            .find(|e| e.0 == *kind && e.1 == *id && e.2 == *hash)
                            .map(|e| RegistryEntryEnvelope {
                                kind: e.0,
                                id: e.1.clone(),
                                hash: e.2.clone(),
                                canonical_json: e.3.clone(),
                            }),
                    },
                }
            },
        )?;
        let runtime = ComponentRuntime::new(owner);
        contract.validate()?;
        let compact = crate::compact_pose::Contract {
            from_frame: (&contract.from_frame).into(),
            to_frame: (&contract.to_frame).into(),
            clock: contract.clock.clone(),
            session_id: contract.session_id.clone(),
            lifetime: crate::compact_pose::Lifetime::Sampled,
        };
        let component = runtime.component(ComponentSpec::new("bbos-pose").observable(
            ObservableContract {
                name: "poses".into(),
                datatype: crate::compact_pose::SCHEMA.into(),
                schema: crate::compact_pose::SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            },
        ))?;
        let output: ConfiguredObservable<crate::compact_pose::Update> = component
            .configured_observable(
                ConfiguredObservableSpec::new(
                    "poses",
                    compact.output_id(),
                    contract.clock.clone(),
                    PayloadContract::Structured(StructuredPayloadContract {
                        modality: "pose".into(),
                        datatype: crate::compact_pose::SCHEMA.into(),
                        schema: crate::compact_pose::SCHEMA.into(),
                        observes: "robot_base_pose_in_odometry".into(),
                        unit: None,
                    }),
                )
                .in_registered_frame(contract.to_frame.clone()),
            )?;
        component.expose()?;
        compact.validate_output(&output.manifest())?;
        let capture = runtime.capture_buffer_with_metadata(
            "bbos-pose-history",
            &output,
            BufferLimits::entries(300),
            |o| serde_json::to_vec(o).map_or(usize::MAX, |v| v.len()),
            move |o| {
                o.payload.validate().map_err(|e| e.to_string())?;
                Ok(Some(CatalogProductMetadata {
                    schema: crate::compact_pose::METADATA.into(),
                    source_sequence: o.sequence,
                    value: serde_json::to_value(&compact).map_err(|e| e.to_string())?,
                }))
            },
        )?;
        let endpoint = ComponentProtocolEndpoint::mount(peer.protocols(), runtime.clone())?;
        endpoint.export_product(&capture.product())?;
        Ok(Self {
            runtime,
            contract,
            definitions,
            _component: component,
            output,
            _capture: capture,
            endpoint,
            registry,
            last: 0,
            source: None,
        })
    }

    pub fn publish(&mut self, sample: Sample) -> Result<()> {
        if let Some(source) = sample.source_id {
            ensure!(
                self.source.is_none_or(|old| old == source),
                "BBOS pose writer changed; restart diagnostic for a fresh frame/session"
            );
            self.source = Some(source);
        }
        // Never replay stale pose timestamps or invent a pose during tracking loss.
        if sample.timestamp_ns <= self.last {
            return Ok(());
        }
        let update = crate::compact_pose::Update {
            tracking: match sample.pose {
                Some(pose) => Tracking::Valid { pose },
                None => Tracking::Lost,
            },
        };
        update.validate()?;
        self.output
            .publish(sample.timestamp_ns, std::sync::Arc::new(update))?;
        self.last = sample.timestamp_ns;
        Ok(())
    }
}
