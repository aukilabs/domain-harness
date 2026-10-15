//! Canonical scene publication over the SDK Component protocols. No portal APIs.
use crate::model::{Built, FRAME, SDK_REV, SemanticObject, clock_entry, clock_ref};
use anyhow::{Result, anyhow, bail, ensure};
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ComponentProtocolEndpoint, ObservationStart,
    RemoteProductSubscription,
};
use auki_components::clock::ClockRegistryEntry;
use auki_components::{
    BufferLimits, BufferProductCapture, Component, ComponentRuntime, ComponentSpec,
    ConfiguredObservable, ConfiguredObservableSpec, Exposure, ObservableContract,
    ObservationAccess, PayloadContract, ProductReference, StructuredPayloadContract,
};
use auki_scenegraph::MapSnapshot;
use auki_sdk::{AukiPeer, Multiaddr, PeerId};
use auki_voxel_map::VoxelSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{sync::Arc, time::Instant};

pub const SCENE_SCHEMA: &str = "spatial-harness.canonical-scene/v1";
pub const SCENE_PRODUCT: &str = "canonical-scene";
pub(crate) const MAX_SCENE_BYTES: usize = 24 * 1024 * 1024;

/// Complete replacement state, never additive voxel evidence. Publication time
/// belongs to the boot clock; capture clocks inside snapshots remain unchanged.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanonicalScene {
    pub schema: String,
    pub sdk_revision: String,
    pub authority_peer_id: String,

    pub frame: String,
    pub publication_clock: ClockRegistryEntry,
    pub portals: MapSnapshot,
    pub voxels: VoxelSnapshot,
    pub objects: Vec<SemanticObject>,
    pub registries: Value,
}
impl auki_components::ContractType for CanonicalScene {
    const DATATYPE: &'static str = SCENE_SCHEMA;
}
impl CanonicalScene {
    /// The same scene without its voxel checkpoint bytes, for a header that
    /// carries them separately in binary.
    pub fn without_checkpoint(&self) -> Self {
        Self {
            schema: self.schema.clone(), sdk_revision: self.sdk_revision.clone(),
            authority_peer_id: self.authority_peer_id.clone(), frame: self.frame.clone(),
            publication_clock: self.publication_clock.clone(), portals: self.portals.clone(),
            voxels: auki_voxel_map::VoxelSnapshot {
                definition: self.voxels.definition.clone(), checkpoint: Vec::new(),
                integrated_observations: self.voxels.integrated_observations,
                latest_observation: self.voxels.latest_observation.clone(),
            },
            objects: self.objects.clone(), registries: self.registries.clone(),
        }
    }
}

impl CanonicalScene {
    pub fn validate(&self, expected_peer: &str) -> Result<()> {
        ensure!(
            self.schema == SCENE_SCHEMA && self.sdk_revision == SDK_REV,
            "incompatible scene schema/SDK"
        );
        ensure!(
            self.authority_peer_id == expected_peer,
            "unexpected map authority"
        );
        ensure!(
            self.publication_clock.peer_id == expected_peer,
            "publication clock owner mismatch"
        );
        ensure!(
            self.portals.scenegraph.map.frame.id == self.frame,
            "canonical frame mismatch"
        );
        self.portals.validate()?;
        crate::notes::from_registries(&self.registries, &self.frame)?;
        crate::landmarks::from_registries(&self.registries, &self.frame)?;
        crate::conventions::validate_frame(&self.portals.scenegraph.map.frame)?;
        ensure!(
            self.voxels.definition.portal_map == self.portals.scenegraph.map,
            "voxel and Portal frames differ"
        );
        ensure!(
            if crate::local_mapper::is_local_frame(&self.frame)
                || crate::local_scenes::frame_scene_id(&self.frame).is_some()
            {
                self.portals.scenegraph.map.domain_reference.is_none()
            } else {
                self.portals.scenegraph.map.domain_reference.as_deref()
                    == Some(crate::domain_portals::DOMAIN)
            },
            "scene domain mismatch"
        );
        let expected =
            crate::conventions::registry(&self.voxels.definition.frame.peer_id, &self.frame)?;
        ensure!(
            expected == self.voxels.definition.frame,
            "voxel axis convention mismatch"
        );
        // A phone's scene carries no voxel checkpoint: its voxels come as deltas.
        if !self.voxels.checkpoint.is_empty() {
            self.voxels.accumulator()?;
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= MAX_SCENE_BYTES,
            "scene exceeds publication limit"
        );
        Ok(())
    }
}

/// Public pairing information. It contains no private identity or credential.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeConnection {
    pub schema: String,
    pub peer_id: String,
    pub domain_id: String,
    pub routes: Vec<String>,
    pub frame: String,
    pub product_id: String,
}
impl HomeConnection {
    pub fn local_scene_id(&self) -> Option<&str> {
        crate::local_scenes::frame_scene_id(&self.frame)
    }
    pub fn validate_scene_contract(&self) -> Result<()> {
        if let Some(id) = self.local_scene_id() {
            ensure!(
                self.product_id == crate::local_scenes::mobile_product(id),
                "Scene Product/frame mismatch"
            );
        } else {
            ensure!(
                (crate::conventions::convention(&self.frame).is_ok()
                    || crate::local_mapper::is_local_family(&self.frame))
                    && self.product_id == SCENE_PRODUCT,
                "unexpected Home scene contract"
            );
        }
        Ok(())
    }
    pub fn scoped_component(&self, base: &str) -> String {
        self.local_scene_id()
            .map_or_else(|| base.to_owned(), |id| format!("{base}:{id}"))
    }
    pub fn current(peer: &AukiPeer) -> Result<Self> {
        let routes = peer.protocol_context().routes().snapshot()?;
        let mut connection = Self::direct(peer);
        connection.routes = routes
            .direct_routes
            .iter()
            .map(ToString::to_string)
            .chain(
                routes
                    .relay_routes
                    .iter()
                    .map(|r| r.routes.tcp().to_string()),
            )
            .collect();
        if connection.routes.is_empty() {
            connection.routes = peer
                .listen_addresses()
                .iter()
                .map(ToString::to_string)
                .collect();
        }
        Ok(connection)
    }
    pub fn direct(peer: &AukiPeer) -> Self {
        Self {
            schema: "spatial-harness.home-connection/v1".into(),
            peer_id: peer.peer_id().to_string(),
            domain_id: peer.domain_id().to_string(),
            routes: peer
                .listen_addresses()
                .iter()
                .map(ToString::to_string)
                .collect(),
            frame: FRAME.into(),
            product_id: SCENE_PRODUCT.into(),
        }
    }
    pub fn validate(&self, peer: &AukiPeer) -> Result<PeerId> {
        ensure!(
            self.schema == "spatial-harness.home-connection/v1",
            "unknown connection profile"
        );
        ensure!(
            self.domain_id == peer.domain_id().to_string(),
            "Home and runner transport Domains differ"
        );
        self.validate_scene_contract()?;
        ensure!(
            !self.routes.is_empty() && self.routes.len() <= 16,
            "Home profile needs 1–16 routes"
        );
        Ok(self.peer_id.parse()?)
    }
}

pub struct ScenePublisher {
    pub endpoint: Arc<ComponentProtocolEndpoint>,
    capture: Option<BufferProductCapture<CanonicalScene>>,
    output: ConfiguredObservable<CanonicalScene>,
    _component: Component,
    pub runtime: ComponentRuntime,
    peer_id: String,
    clock: ClockRegistryEntry,
    started: Instant,
    last_scene: Option<Vec<u8>>,
    inputs: Option<crate::shared_inputs::InputPublisher>,
    frame_transform: Option<crate::frame_transform::Publisher>,
    live: Option<(
        Component,
        ConfiguredObservable<CanonicalScene>,
        BufferProductCapture<CanonicalScene>,
    )>,
}
impl ScenePublisher {
    pub fn mount(peer: &AukiPeer) -> Result<Self> {
        Self::mount_in_frame(peer, FRAME)
    }
    pub fn mount_in_frame(peer: &AukiPeer, frame: &str) -> Result<Self> {
        Self::mount_with_reference(peer, frame, true)
    }
    /// Keep the shared endpoint/registry host without exposing a deleted legacy
    /// reference scene. Independent local scenes mount on this same endpoint.
    pub fn mount_without_reference(peer: &AukiPeer) -> Result<Self> {
        Self::mount_with_reference(peer, FRAME, false)
    }
    fn mount_with_reference(peer: &AukiPeer, frame: &str, reference: bool) -> Result<Self> {
        let peer_id = peer.peer_id().to_string();
        let runtime = ComponentRuntime::new(peer_id.clone());
        let mut clock = clock_entry(&peer_id, &uuid::Uuid::new_v4().to_string());
        clock.clock_id = "home-publication".into();
        let component = runtime.component(
            ComponentSpec::new("canonical-scene-publisher").observable(ObservableContract {
                name: "scene".into(),
                datatype: SCENE_SCHEMA.into(),
                schema: SCENE_SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }),
        )?;
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "scene",
                format!("scene-{}", clock.session_id),
                clock_ref(&clock),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "scenegraph".into(),
                    datatype: SCENE_SCHEMA.into(),
                    schema: SCENE_SCHEMA.into(),
                    observes: "canonical_scene".into(),
                    unit: Some("meters".into()),
                }),
            )
            .in_registered_frame(crate::conventions::registry(&peer_id, frame)?),
        )?;
        let capture = if reference {
            component.expose()?;
            Some(runtime.capture_buffer(SCENE_PRODUCT, &output, scene_limits(), scene_size)?)
        } else { None };
        let endpoint = ComponentProtocolEndpoint::mount(peer.protocols(), runtime.clone())?;
        // Export only the coherent aggregate. Internal mapper Products are rebuilt
        // by the journal and are not stable network subscription identities.
        let inputs = if let Some(capture) = &capture {
            endpoint.export_product(&capture.product())?;
            Some(crate::shared_inputs::InputPublisher::mount(&runtime, &endpoint, clock_ref(&clock))?)
        } else { None };
        Ok(Self {
            inputs,
            frame_transform: None,
            live: None,
            endpoint: Arc::new(endpoint),
            capture,
            output,
            _component: component,
            runtime,
            peer_id,
            clock,
            started: Instant::now(),
            last_scene: None,
        })
    }
    pub fn enable_annotations(
        &self,
        store: Arc<std::sync::Mutex<crate::model::Store>>,
        changed: Arc<tokio::sync::Notify>,
    ) -> Result<()> {
        self.enable_annotations_with_live(store, changed, None)
    }
    pub fn enable_annotations_with_live(
        &self,
        store: Arc<std::sync::Mutex<crate::model::Store>>,
        changed: Arc<tokio::sync::Notify>,
        live: Option<tokio::sync::watch::Receiver<crate::scene_mirror::RemoteScene>>,
    ) -> Result<()> {
        self.enable_annotations_with_volumes(store,changed,live,None)
    }
    pub fn enable_annotations_with_volumes(
        &self,
        store: Arc<std::sync::Mutex<crate::model::Store>>,
        changed: Arc<tokio::sync::Notify>,
        live: Option<tokio::sync::watch::Receiver<crate::scene_mirror::RemoteScene>>,
        scenes: Option<Arc<crate::local_scenes::LocalScenes>>,
    ) -> Result<()> {
        use auki_components::{ContractType, InvocationError, OperableContract};
        let component =
            self.runtime
                .component(ComponentSpec::new(crate::mobile_notes::COMPONENT).operable(
                    OperableContract {
                        name: crate::mobile_notes::OPERATION.into(),
                        instruction: crate::mobile_notes::PlaceNote::DATATYPE.into(),
                        result: crate::notes::TextNote::DATATYPE.into(),
                        exposure: Exposure::Cluster,
                    },
                ).operable(crate::mobile_notes::change_contract()))?;
        let change_store = store.clone();
        let change_notify = changed.clone();
        let memo = std::sync::Mutex::new(crate::mobile_notes::ChangeMemo::default());
        let change = component.operable("change", |ctx| !ctx.caller_peer_id.is_empty(),
            move |ctx, request: crate::mobile_notes::ChangeNote| {
                memo.lock().map_err(|_| InvocationError::TargetUnavailable)?.run(&ctx.caller_peer_id, &request, || {
                let mut store = change_store.lock().map_err(|_| InvocationError::TargetUnavailable)?;
                let result = (|| -> Result<_> {
                    if matches!(request.action,crate::mobile_notes::NoteAction::PutVolume{..}|crate::mobile_notes::NoteAction::DeleteVolume{..})
                        && let Some(scenes)=&scenes {
                        let result=scenes.volumes.sdk_change("canonical",crate::model::FRAME,&request)?;
                        store.built.registries["semantic_volumes"]=serde_json::to_value(scenes.volumes.get("canonical",crate::model::FRAME)?)?;
                        return Ok(result);
                    }
                    let notes = crate::notes::from_registries(&store.built.registries, crate::model::FRAME)?;
                    let (notes, result) = crate::mobile_notes::apply_change(&notes, &request, crate::model::FRAME)?;
                    crate::model::persist(&store.path.with_extension("notes.json"), &notes)?;
                    store.built.registries["text_notes"] = serde_json::to_value(notes)?;
                    Ok(result)
                })().map_err(|e| InvocationError::Rejected(e.to_string()))?;
                change_notify.notify_one();
                Ok(result)
                }).map_err(|e| InvocationError::Rejected(e.to_string()))
            })?;
        let operation = component.operable(
            crate::mobile_notes::OPERATION,
            // Home's collaboration policy permits authenticated Domain peers to
            // append bounded text notes. This operation cannot edit other data.
            |context| !context.caller_peer_id.is_empty(),
            move |_, request: crate::mobile_notes::PlaceNote| {
                let geometry = live.as_ref().and_then(|live| {
                    let state = live.borrow();
                    (state.status_view()["status"] == "connected")
                        .then(|| state.scene.clone())
                        .flatten()
                });
                let mut store = store
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?;
                let note =
                    crate::mobile_notes::place_with_live(&mut store, request, geometry.as_ref())
                        .map_err(|e| InvocationError::Rejected(e.to_string()))?;
                changed.notify_one();
                Ok(note)
            },
        )?;
        component.expose()?;
        self.endpoint.export_operable(&operation)?;
        self.endpoint.export_operable(&change)?;
        Ok(())
    }
    pub fn enable_phone_depth(
        &self,
        inbox: crate::mobile_depth::SharedInbox,
        canonical: MapSnapshot,
    ) -> Result<()> {
        use auki_components::{ContractType, InvocationError, OperableContract};
        let component =
            self.runtime
                .component(ComponentSpec::new(crate::mobile_depth::COMPONENT).operable(
                    OperableContract {
                        name: "ingest".into(),
                        instruction: crate::mobile_depth::Envelope::DATATYPE.into(),
                        result: "bool".into(),
                        exposure: Exposure::Cluster,
                    },
                ))?;
        let operation = component.operable(
            "ingest",
            |context| !context.caller_peer_id.is_empty(),
            move |context, envelope: crate::mobile_depth::Envelope| {
                crate::home_pose_policy::unsupported("Home depth fusion").map_err(|e| InvocationError::Rejected(e.to_string()))?;
                inbox
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?
                    .accept(&context.caller_peer_id, envelope, &canonical)
                    .map_err(|e| InvocationError::Rejected(e.to_string()))
            },
        )?;
        component.expose()?;
        self.endpoint.export_operable(&operation)?;
        Ok(())
    }
    pub fn enable_phone_presence(
        &self,
        store: Arc<std::sync::Mutex<crate::model::Store>>,
        changed: Arc<tokio::sync::Notify>,
    ) -> Result<Arc<std::sync::Mutex<crate::mobile_pose::Presence>>> {
        use auki_components::{ContractType, InvocationError, OperableContract};
        let presence = Arc::new(std::sync::Mutex::new(
            crate::mobile_pose::Presence::default(),
        ));
        let cache = presence.clone();
        let component =
            self.runtime
                .component(ComponentSpec::new(crate::mobile_pose::COMPONENT).operable(
                    OperableContract {
                        name: "update".into(),
                        instruction: crate::mobile_pose::Update::DATATYPE.into(),
                        result: "bool".into(),
                        exposure: Exposure::Cluster,
                    },
                ))?;
        let operation = component.operable(
            "update",
            |context| !context.caller_peer_id.is_empty(),
            move |context, update: crate::mobile_pose::Update| {
                crate::home_pose_policy::unsupported("Home phone presence").map_err(|e| InvocationError::Rejected(e.to_string()))?;
                let mut cache = cache
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?;
                cache
                    .accept(&context.caller_peer_id, update, Instant::now())
                    .map_err(|e| InvocationError::Rejected(e.to_string()))?;
                let mut store = store
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?;
                crate::mobile_pose::sync(&mut store, &cache, Instant::now());
                changed.notify_one();
                Ok(true)
            },
        )?;
        component.expose()?;
        self.endpoint.export_operable(&operation)?;
        Ok(presence)
    }
    pub fn enable_home_live(&mut self) -> Result<()> {
        let component =
            self.runtime
                .component(ComponentSpec::new("home-live-publisher").observable(
                    ObservableContract {
                        name: "scene".into(),
                        datatype: SCENE_SCHEMA.into(),
                        schema: SCENE_SCHEMA.into(),
                        access: vec![ObservationAccess::FollowNew],
                        exposure: Exposure::Cluster,
                    },
                ))?;
        let output = component.configured_observable(ConfiguredObservableSpec::new(
            "scene",
            format!("home-live-{}", self.clock.session_id),
            clock_ref(&self.clock),
            PayloadContract::Structured(StructuredPayloadContract {
                modality: "scenegraph".into(),
                datatype: SCENE_SCHEMA.into(),
                schema: SCENE_SCHEMA.into(),
                observes: "home_live_observations".into(),
                unit: Some("meters".into()),
            }),
        ))?;
        component.expose()?;
        let capture =
            self.runtime
                .capture_buffer("home-live-scene", &output, scene_limits(), scene_size)?;
        self.endpoint.export_product(&capture.product())?;
        self.live = Some((component, output, capture));
        Ok(())
    }
    pub fn publish_home_live(&self, scene: &CanonicalScene) -> Result<()> {
        let (_, output, capture) = self
            .live
            .as_ref()
            .ok_or_else(|| anyhow!("Home live publisher unavailable"))?;
        let mut scene = scene.clone();
        scene.publication_clock = self.clock.clone();
        scene.validate(&self.peer_id)?;
        output.publish(self.started.elapsed().as_nanos() as u64, Arc::new(scene))?;
        ensure!(
            capture.errors().is_empty(),
            "Home live scene retention failed"
        );
        Ok(())
    }
    pub fn publish(&mut self, built: &Built) -> Result<bool> {
        let capture = self.capture.as_ref().ok_or_else(|| anyhow!("Reference scene was deleted"))?;
        let scene = CanonicalScene {
            schema: SCENE_SCHEMA.into(),
            sdk_revision: SDK_REV.into(),
            authority_peer_id: self.peer_id.clone(),

            frame: built.portals.scenegraph.map.frame.id.clone(),
            publication_clock: self.clock.clone(),
            portals: built.portals.clone(),
            voxels: built.voxels.clone(),
            objects: built.view.objects.clone(),
            registries: built.registries.clone(),
        };
        scene.validate(&self.peer_id)?;
        let content = serde_json::to_vec(&scene)?;
        if self.last_scene.as_ref() == Some(&content) {
            return Ok(false);
        }
        self.output
            .publish(self.started.elapsed().as_nanos() as u64, Arc::new(scene))?;
        ensure!(
            capture.errors().is_empty(),
            "scene capture failed: {:?}",
            capture.errors()
        );
        self.last_scene = Some(content);
        Ok(true)
    }
    pub fn publish_local(
        &mut self,
        local: &crate::local_mapper::LocalMap,
    ) -> Result<Option<CanonicalScene>> {
        let capture = self.capture.as_ref().ok_or_else(|| anyhow!("Reference scene was deleted"))?;
        if self.frame_transform.is_none() {
            self.frame_transform = Some(crate::frame_transform::Publisher::mount(
                &self.runtime, &self.endpoint, clock_ref(&self.clock),
            )?);
        }
        let transform = self.frame_transform.as_ref().unwrap();
        let published_transform = transform.publish(local, &self.peer_id, self.started.elapsed().as_nanos() as u64)?;
        let Some(voxels) = &local.voxels else {
            return Ok(None);
        };
        let portals = local.portals()?;
        let mut registries = local.metadata();
        registries["frame_transform_product"] = serde_json::to_value(transform.reference())?;
        registries["frame_transform"] = serde_json::to_value(published_transform)?;
        registries["landmarks"] = serde_json::to_value(local.landmarks(&portals)?)?;
        let scene = CanonicalScene {
            schema: SCENE_SCHEMA.into(),
            sdk_revision: SDK_REV.into(),
            authority_peer_id: self.peer_id.clone(),

            frame: local.frame.clone(),
            publication_clock: self.clock.clone(),
            portals,
            voxels: voxels.clone(),
            objects: vec![],
            registries,
        };
        scene.validate(&self.peer_id)?;
        self.output.publish(
            self.started.elapsed().as_nanos() as u64,
            Arc::new(scene.clone()),
        )?;
        ensure!(
            capture.errors().is_empty(),
            "local scene retention failed"
        );
        Ok(Some(scene))
    }
    pub fn publish_inputs(&mut self, inputs: &[crate::runner_voxels::MappingInput]) -> Result<()> {
        self.inputs
            .as_mut().ok_or_else(|| anyhow!("Reference scene was deleted"))?
            .publish(inputs, self.started.elapsed().as_nanos() as u64)
    }
    pub async fn close(self) -> Result<()> {
        if let Some(capture) = &self.capture { capture.cancel(); }
        Arc::try_unwrap(self.endpoint)
            .map_err(|_| anyhow!("endpoint still in use at shutdown"))?
            .close()
            .await?;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn serve_scene(
    running: crate::peer::RunningPeer,
    store: Arc<std::sync::Mutex<crate::model::Store>>,
    local_scenes: Arc<crate::local_scenes::LocalScenes>,
    runner_names: std::collections::BTreeMap<String, String>,
    mut changes: tokio::sync::watch::Receiver<()>,
    status: tokio::sync::watch::Sender<Value>,
    runner: Option<crate::runner_pose::RunnerConfig>,
    runner_scene: Option<HomeConnection>,
    remote_state: tokio::sync::watch::Sender<crate::scene_mirror::RemoteScene>,
    extra_runners: Vec<(
        HomeConnection,
        tokio::sync::watch::Sender<crate::scene_mirror::RemoteScene>,
    )>,
    home_live: tokio::sync::watch::Sender<crate::scene_mirror::RemoteScene>,
    _runner_state: tokio::sync::watch::Sender<crate::runner_pose::RunnerState>,
    mapping: crate::runner_voxels::MappingTarget,
    previews: tokio::sync::mpsc::Receiver<crate::sensor_preview::Request>,
    controls: tokio::sync::mpsc::Receiver<crate::component_control::Request>,
    pointing: tokio::sync::mpsc::Receiver<crate::robot_pointing::Invoke>,
    navigation: tokio::sync::mpsc::Receiver<crate::robot_navigation::Invoke>,
    occupancy: crate::robot_occupancy::Cache,
    robot_states: crate::robot_states::RobotStates,
    ar_frames: crate::ar_preview::Frames,
    shutdown: tokio_util::sync::CancellationToken,
) -> Result<()> {
    let monitor_stop = shutdown.child_token();
    mapping.status.send_replace(if local_scenes.reference_enabled {
        serde_json::json!({"status":"reference","detail":"Canonical portal record; live geometry has its own scenegraph"})
    } else {
        serde_json::json!({"status":"local_scenes","detail":"Legacy reference deleted; mapping uses independent local scenes"})
    });
    let remote_portals = store
        .lock()
        .map_err(|_| anyhow!("store unavailable"))?
        .built
        .portals
        .clone();
    let remote = async { if local_scenes.reference_enabled { crate::scene_mirror::monitor(
        &running.peer,
        runner_scene.as_ref(),
        &remote_portals,
        &remote_state,
        &monitor_stop,
    ).await; } };
    let extra_monitors = futures::future::join_all(extra_runners.iter().filter(|_| local_scenes.reference_enabled).map(|(config, state)| {
        crate::scene_mirror::monitor(
            &running.peer,
            Some(config),
            &remote_portals,
            state,
            &monitor_stop,
        )
    }));
    let home_sources: Vec<_> = runner_scene
        .iter()
        .cloned()
        .chain(extra_runners.iter().map(|(profile, _)| profile.clone()))
        .collect();
    let phone_depth = Arc::new(std::sync::Mutex::new(crate::mobile_depth::Inbox::default()));
    let home_mapper = async { if local_scenes.reference_enabled { crate::home_mapper::monitor(
        &running.peer,
        &home_sources,
        &remote_portals,
        store.clone(),
        runner_names,
        &home_live,
        &ar_frames,
        phone_depth.clone(),
        &monitor_stop,
    ).await; } };
    let robot_occupancy =
        crate::robot_occupancy::monitor(&running.peer, &home_sources, &occupancy, &monitor_stop);
    let (catalog_tx, mut catalog_rx) =
        tokio::sync::watch::channel(crate::device_catalogs::Catalogs::new());
    let catalog_targets: Vec<_> = home_sources
        .iter()
        .map(|p| (p.peer_id.clone(), p.routes.clone()))
        .chain(runner.iter().map(|r| (r.peer_id.clone(), r.routes.clone())))
        .collect();
    let registry_definitions=crate::product_registry::Definitions::default();
    for snapshot in local_scenes.snapshots.values(){crate::product_registry::frame(&registry_definitions,&crate::conventions::registry(&running.peer.peer_id().to_string(),&snapshot.scenegraph.map.frame.id)?)?;}
    let catalogs =
        crate::device_catalogs::monitor(&running.peer, &registry_definitions, &local_scenes, &catalog_targets, catalog_tx, &monitor_stop);
    let preview_catalogs = catalog_rx.clone();
    let publish = async {
        let publisher = if local_scenes.reference_enabled { ScenePublisher::mount(&running.peer) }
            else { ScenePublisher::mount_without_reference(&running.peer) };
        let result = match publisher {
            Err(error) => Err(error),
            Ok(mut publisher) => {
                crate::product_registry::clock(&registry_definitions,&publisher.clock)?;
                let _registry=crate::product_registry::mount(&running.peer,publisher.runtime.clone(),registry_definitions.clone())?;
                let work = async {
                if local_scenes.reference_enabled { publisher.enable_home_live()?; }
                let _local_scene_components = local_scenes.mount(
                    &publisher.runtime, &publisher.endpoint,
                    clock_ref(&publisher.clock), publisher.started,
                )?;
                let mut traffic = crate::peer_traffic::Publisher::mount(&publisher.runtime, &publisher.endpoint, &publisher.clock, publisher.started)?;
                let mut local_mobile = crate::local_mobile::Publisher::mount_all(
                    &publisher.runtime, &publisher.endpoint, local_scenes.clone(),
                    publisher.clock.clone(), publisher.started,
                )?;
                crate::mobile_local::mount_receiver(&publisher.runtime, &publisher.endpoint)?;
                // Opt in with a measured floor height in the live map's up axis.
                // Bind the same retained Product that peers subscribe to.
                let mut occupancy_mapper = if !local_scenes.reference_enabled { None } else { match std::env::var("SPATIAL_OCCUPANCY_FLOOR_M") {
                    Ok(floor) => Some(crate::occupancy_mapper::OccupancyMapper::mount(
                        &publisher.runtime, &publisher.endpoint,
                        &publisher.live.as_ref().unwrap().2.product(),
                        &publisher.peer_id, clock_ref(&publisher.clock),
                        crate::occupancy_mapper::Config::new(floor.parse()?),
                    )?),
                    Err(std::env::VarError::NotPresent) => None,
                    Err(e) => return Err(e.into()),
                } };
                let annotation_changes = Arc::new(tokio::sync::Notify::new());
                let phone_presence = if local_scenes.reference_enabled {
                    publisher.enable_phone_depth(phone_depth.clone(),remote_portals.clone())?;
                    publisher.enable_annotations_with_volumes(store.clone(), annotation_changes.clone(), Some(home_live.subscribe()),Some(local_scenes.clone()))?;
                    Some(publisher.enable_phone_presence(store.clone(), annotation_changes.clone())?)
                } else { None };
                let mut presence_tick = tokio::time::interval(std::time::Duration::from_millis(500));
                let mut live_updates=home_live.subscribe();
                let mut last_live=None;
                let mut peer_status = running.peer.subscribe_status();
                let mut routes = running.peer.protocol_context().routes().subscribe();
                loop {
                    if let Some(phone_presence) = &phone_presence { let presence = phone_presence.lock().map_err(|_|anyhow!("presence unavailable"))?;
                        let mut store=store.lock().map_err(|_|anyhow!("store unavailable"))?;
                        crate::mobile_pose::sync(&mut store, &presence, Instant::now());
                        publisher.publish_inputs(store.mapping_inputs())?;
                        publisher.publish(&store.built)?;
                    }
                    if let Some(scene)=live_updates.borrow().scene.as_ref() {
                        let key=serde_json::to_vec(scene)?;
                        if last_live.as_ref()!=Some(&key) {publisher.publish_home_live(scene)?;last_live=Some(key);}
                    }
                    if let Some(mapper) = &mut occupancy_mapper { mapper.tick()?; }
                    for scene in &mut local_mobile { scene.tick()?; }
                    traffic.tick()?;
                    let mut connection = HomeConnection::current(&running.peer)?;
                    if !local_scenes.reference_enabled {
                        let id = local_scenes.preferred_id().or_else(||local_scenes.snapshots.keys().next().map(|id|format!("local:{id}")))
                            .ok_or_else(||anyhow!("Home needs an active scene for its connection profile"))?;
                        connection = local_scenes.connection(id.strip_prefix("local:").unwrap(), connection)?;
                    }
                    let peers = crate::device_catalogs::device_rows(&running.peer, &catalog_rx.borrow());
                    status.send_replace(serde_json::json!({"enabled":true,
                        "status":format!("{:?}", running.peer.status()), "connection":connection,
                        "peers":peers,"catalog":publisher.runtime.catalog().snapshot(),
                        "occupancy_mapper":occupancy_mapper.as_ref().map(|m| serde_json::json!({
                            "product":crate::occupancy_mapper::PRODUCT,
                            "status":m.state().status,"detail":m.state().detail
                        }))}));
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        exit = running.peer.wait_stopped() => bail!("Home peer stopped: {exit:?}"),
                        changed = changes.changed() => { changed?; },
                        _ = annotation_changes.notified() => {},
                        _ = presence_tick.tick() => {},
                        changed = live_updates.changed() => { changed?; },
                        changed = peer_status.changed() => { changed?; },
                        changed = routes.changed() => { changed?; },
                        changed = catalog_rx.changed() => { changed?; },
                    }
                }
                Ok::<_, anyhow::Error>(())
            }
            .await;
                let cleanup = publisher.close().await;
                work.and(cleanup)
            }
        };
        monitor_stop.cancel();
        result
    };
    let held = crate::frame_alignment::HeldTransforms::default();
    let controls = crate::component_control::serve(&running.peer,preview_catalogs.clone(),controls,&monitor_stop);
    let held_subscriptions = held.serve(&running.peer, preview_catalogs.clone(), &monitor_stop);
    let robot_bodies = robot_states.serve(&running.peer, preview_catalogs.clone(), &monitor_stop);
    let declared_depth = crate::declared_depth::serve(&running.peer, preview_catalogs.clone(), &local_scenes, &held, &monitor_stop);
    let previews = crate::sensor_preview::serve(
        &running.peer,
        preview_catalogs,
        previews,
        home_live.subscribe(),
        &ar_frames,
        &local_scenes,
        &held,
        &monitor_stop,
    );
    let pointing = crate::robot_pointing::serve(&running.peer, pointing, &monitor_stop);
    let navigation = crate::robot_navigation::serve(&running.peer, navigation, &monitor_stop);
    let (result, _, _, _, _, _, _, _, _, _, _, _, _) = tokio::join!(
        publish,
        remote,
        previews,
        home_mapper,
        extra_monitors,
        robot_occupancy,
        catalogs,
        declared_depth,
        held_subscriptions,
        controls,
        pointing,
        navigation,
        robot_bodies
    );
    shutdown.cancel();
    status.send_replace(serde_json::json!({"enabled":true,"status":"Stopped"}));
    let cleanup = running.shutdown().await;
    result.and(cleanup)
}
fn scene_size(scene: &CanonicalScene) -> usize {
    serde_json::to_vec(scene)
        .map(|b| b.len())
        .unwrap_or(MAX_SCENE_BYTES + 1)
}
fn scene_limits() -> BufferLimits {
    BufferLimits {
        max_entries: Some(2),
        max_bytes: Some(2 * MAX_SCENE_BYTES),
        target_duration: None,
    }
}

/// Exact expected Peer ID is enforced by the SDK's authenticated stream. Route
/// failures are bounded; callers explicitly reconnect and rediscover on failure.
pub async fn subscribe_scene(
    peer: &AukiPeer,
    home: &HomeConnection,
) -> Result<RemoteProductSubscription<CanonicalScene>> {
    subscribe_scene_product(peer, home, SCENE_PRODUCT).await
}

pub async fn subscribe_scene_product(
    peer: &AukiPeer,
    home: &HomeConnection,
    product: &str,
) -> Result<RemoteProductSubscription<CanonicalScene>> {
    let expected = home.validate(peer)?;
    let client = ComponentProtocolClient::new(peer.protocols());
    let mut failures = Vec::new();
    for route in &home.routes {
        let route: Multiaddr = route.parse()?;
        let attempt = async {
            let CatalogResponse::Snapshot { snapshot } =
                client.catalog_exact(expected, route.clone(), None).await?
            else {
                bail!("initial catalog was not a snapshot");
            };
            let entry = snapshot
                .products
                .iter()
                .find(|p| p.manifest.product_id == product)
                .ok_or_else(|| anyhow!("Home does not export the canonical scene Product"))?;
            ensure!(
                entry.manifest.peer_id == home.peer_id,
                "catalog Product owner mismatch"
            );
            let subscription = client
                .subscribe_product_exact(
                    expected,
                    route,
                    ProductReference {
                        peer_id: entry.manifest.peer_id.clone(),
                        product_id: entry.manifest.product_id.clone(),
                        manifest_hash: entry.manifest_hash.clone(),
                    },
                    ObservationStart::LatestExisting,
                    scene_limits(),
                    scene_size,
                )
                .await?;
            ensure!(
                subscription.product().producer.payload.schema() == SCENE_SCHEMA,
                "unexpected scene payload schema"
            );
            Ok::<_, anyhow::Error>(subscription)
        };
        match tokio::time::timeout(std::time::Duration::from_secs(20), attempt).await {
            Ok(Ok(s)) => return Ok(s),
            Ok(Err(e)) => failures.push(e.to_string()),
            Err(_) => failures.push("route deadline exceeded".into()),
        }
    }
    bail!("Home connection failed: {}", failures.join("; "))
}

/// Bounded, authenticated one-shot fetch. No portal backend APIs.
/// Uses the selected scene Product, including standalone Home reference scenes.
pub async fn fetch_home_portals(peer: &AukiPeer, home: &HomeConnection) -> Result<MapSnapshot> {
    let stop = tokio_util::sync::CancellationToken::new();
    let mut received = None;
    tokio::time::timeout(std::time::Duration::from_secs(45),
        crate::mobile::receive_portals(peer, home, &stop, Some(std::time::Duration::from_millis(50)), |value| {
            received = Some(value);
            stop.cancel();
        })).await??;
    let value = received.ok_or_else(|| anyhow!("Home did not publish a Portal reference"))?;
    let graph = value.get("scenegraph").ok_or_else(|| anyhow!("Home reference missing scenegraph"))?;
    MapSnapshot::new(serde_json::from_value(graph.clone())?).map_err(Into::into)
}

/// Bounded, authenticated one-shot fetch of a complete canonical scene.
pub async fn fetch_home_scene(peer: &AukiPeer, home: &HomeConnection) -> Result<CanonicalScene> {
    let mut subscription = subscribe_scene(peer, home).await?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            match subscription.next().await? {
                Some(auki_component_protocol::RemoteObservationEvent::Observation(o)) => {
                    let scene = (*o.payload).clone();
                    scene.validate(&home.peer_id)?;
                    ensure!(
                        scene.frame == home.frame,
                        "Home frame differs from connection profile"
                    );
                    ensure!(
                        crate::model::clock_ref(&scene.publication_clock)
                            == subscription.product().producer.clock,
                        "Home clock mismatch"
                    );
                    return Ok(scene);
                }
                Some(auki_component_protocol::RemoteObservationEvent::Gap(_)) => {}
                _ => bail!("Home scene stream ended"),
            }
        }
    })
    .await;
    let cleanup = subscription.close().await;
    let scene = result??;
    cleanup?;
    Ok(scene)
}
