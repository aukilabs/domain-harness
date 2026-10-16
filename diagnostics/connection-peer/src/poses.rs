use anyhow::{Result, ensure};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;
use auki_odometry::{Pose, PoseContract, Tracking};
use auki_protocols::registry::{RegistryEndpoint, v3::*};
use auki_registry::{CoordinateConvention, FrameRegistryEntry};
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
    _capture: Option<BufferProductCapture<crate::compact_pose::Update>>,
    pub endpoint: ComponentProtocolEndpoint,
    pub registry: RegistryEndpoint,
    pub session_clock: auki_time::SessionClock,
    pub native_clock: crate::capture_clock::NativeCaptureClock,
    last: u64,
    last_native: u64,
    time_transform: crate::capture_time::Publisher,
    source: Option<u32>,
}

impl Publisher {
    pub fn registered_clock(&self) -> Result<crate::clock_sync::RegisteredClock> {
        crate::clock_sync::RegisteredClock::local(&self.time_transform)
    }
    pub fn product(&self) -> Option<RetainedProduct<crate::compact_pose::Update>> {
        self._capture.as_ref().map(BufferProductCapture::product)
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
        Self::mount_mode(peer, true)
    }
    /// The connection-only diagnostic still advertises its clock, but not a
    /// pose Product with no sensor producing observations.
    pub fn mount_mode(peer: &AukiPeer, publish_poses: bool) -> Result<Self> {
        let owner = peer.peer_id().to_string();
        let session = uuid::Uuid::new_v4().to_string();
        let session_clock = auki_time::SessionClock::new(&owner, &session, "capture");
        let clock = session_clock.registry_entry();
        let native_clock = crate::capture_clock::NativeCaptureClock::new(session_clock.clone());
        native_clock.sample()?;
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
        compact.validate_output(&output.manifest())?;
        // SDK captures expose their Product through the Catalog. A mode with
        // no pose sensor must not construct that capture at all.
        let capture = if publish_poses {
            component.expose()?;
            Some(runtime.capture_buffer_with_metadata(
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
            )?)
        } else { None };
        let endpoint = ComponentProtocolEndpoint::mount(peer.protocols(), runtime.clone())?;
        if let Some(capture) = &capture { endpoint.export_product(&capture.product())?; }
        let mut time_transform = crate::capture_time::Publisher::mount(&runtime, &endpoint, clock)?;
        time_transform.sample(&crate::capture_time::SessionUtcClock(session_clock.clone()))?;
        let utc = &time_transform.contract.to_clock;
        definitions.lock().unwrap().push((RegistryKind::Clock, utc.clock_id.clone(), utc.hash(),
            String::from_utf8(utc.canonical_bytes())?));
        Ok(Self {
            runtime,
            contract,
            definitions,
            _component: component,
            output,
            _capture: capture,
            endpoint,
            registry,
            session_clock,
            native_clock,
            last: 0,
            last_native: 0,
            time_transform,
            source: None,
        })
    }

    pub fn publish(&mut self, sample: Sample) -> Result<()> {
        ensure!(self._capture.is_some(), "Pose sensor is disabled in connection-only mode");
        if let Some(source) = sample.source_id {
            ensure!(
                self.source.is_none_or(|old| old == source),
                "BBOS pose writer changed; restart diagnostic for a fresh frame/session"
            );
            self.source = Some(source);
        }
        if sample.timestamp_ns <= self.last_native { return Ok(()); }
        let Some(timestamp_ns) = self.native_clock.convert(sample.timestamp_ns)? else { return Ok(()); };
        // Never replay stale pose timestamps or invent a pose during tracking loss.
        if timestamp_ns <= self.last {
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
            .publish(timestamp_ns, std::sync::Arc::new(update))?;
        self.last = timestamp_ns;
        self.last_native = sample.timestamp_ns;
        Ok(())
    }

    pub fn sample_clocks(&mut self) -> Result<()> {
        self.native_clock.sample()?;
        self.time_transform.sample(&crate::capture_time::SessionUtcClock(self.session_clock.clone()))?;
        Ok(())
    }
}
