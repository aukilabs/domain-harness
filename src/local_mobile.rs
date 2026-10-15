//! Scene-scoped phone collaboration through the standard SDK Catalog,
//! Products and operations. The optional voxel layer is absent until a real
//! measurement arrives; no placeholder sensor or synthetic observation.
use crate::{
    local_scenes::LocalScenes,
    network::{CanonicalScene, HomeConnection},
    notes::Notes,
};
use anyhow::{Result, anyhow, bail, ensure};
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ComponentProtocolEndpoint, ObservationStart,
    RemoteObservationEvent,
};
use auki_components::*;
use auki_scenegraph::MapSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const SCHEMA: &str = "spatial-harness.mobile-scene/v1";
const MAX_BYTES: usize = 24 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct Scene {
    pub schema: String,
    pub authority_peer_id: String,
    pub publication_clock: clock::ClockRegistryEntry,
    pub portals: MapSnapshot,
    pub geometry: Option<CanonicalScene>,
    pub notes: Notes,
    #[serde(default, skip_serializing_if = "Option::is_none", serialize_with="legacy_volume_layer")]
    pub semantic_volumes: Option<crate::semantic_volumes::Volumes>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<crate::semantic_volumes::SemanticVolume>,
    pub presence: Value,
    #[serde(default)]
    pub robot_presence: Value,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub paths: Option<crate::scene_paths::Paths>,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub paths_product: Option<ProductReference>,
}
impl ContractType for Scene {
    const DATATYPE: &'static str = SCHEMA;
}
fn legacy_volume_layer<S:serde::Serializer>(volumes:&Option<crate::semantic_volumes::Volumes>,s:S)->std::result::Result<S::Ok,S::Error> {
    let mut value=serde_json::to_value(volumes).map_err(serde::ser::Error::custom)?;
    if let Some(items)=value["items"].as_array_mut() { for item in items { item.as_object_mut().unwrap().remove("semantic_type"); } }
    value.serialize(s)
}
fn size(s: &Scene) -> usize {
    pack(s).map_or(usize::MAX, |v| v.len())
}

/// The scene on the wire to phones. Its voxel map is an SDK checkpoint of
/// raw bytes, which JSON would write as a list of numbers (about four
/// characters a byte, 2.5 MB a second at 17k voxels). Instead: "MSC1", the
/// rest of the scene as a u32-length JSON header, then the checkpoint bytes.
pub const ENCODING: &str = "spatial-harness.mobile-scene-packed/v1";
pub fn pack(scene: &Scene) -> Result<Vec<u8>> {
    // Phones take voxels from their own delta Product; the scene carries none.
    let checkpoint: &[u8] = &[];
    let header = serde_json::to_vec(&Scene {
        geometry: scene.geometry.as_ref().map(crate::network::CanonicalScene::without_checkpoint),
        ..scene.shallow()
    })?;
    ensure!(header.len() <= MAX_BYTES && checkpoint.len() <= MAX_BYTES, "Mobile scene too large");
    let mut bytes = Vec::with_capacity(8 + header.len() + checkpoint.len());
    bytes.extend_from_slice(b"MSC1");
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(checkpoint);
    Ok(bytes)
}
pub fn unpack(bytes: &[u8]) -> Result<Scene> {
    ensure!(bytes.len() >= 8 && &bytes[..4] == b"MSC1", "Invalid mobile scene packet");
    let n = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    ensure!(n <= MAX_BYTES && bytes.len() >= 8 + n, "Invalid mobile scene header");
    let mut scene: Scene = serde_json::from_slice(&bytes[8..8 + n])?;
    let checkpoint = &bytes[8 + n..];
    match scene.geometry.as_mut() {
        Some(g) => g.voxels.checkpoint = checkpoint.to_vec(),
        None => ensure!(checkpoint.is_empty(), "Voxel bytes without geometry"),
    }
    Ok(scene)
}
pub fn encoder() -> auki_component_protocol::ObservationEncoder<Scene> {
    auki_component_protocol::ObservationEncoder::new(ENCODING, |s| pack(s).map_err(|e| e.to_string())).expect("static mobile scene codec")
}
pub fn decoder() -> Result<auki_component_protocol::ObservationDecoder<Scene>> {
    Ok(auki_component_protocol::ObservationDecoder::new(ENCODING, |b| unpack(b).map_err(|e| e.to_string()))?)
}
impl Scene {
    /// Everything but the geometry, which can be megabytes.
    fn shallow(&self) -> Self {
        Self {
            schema: self.schema.clone(), authority_peer_id: self.authority_peer_id.clone(),
            publication_clock: self.publication_clock.clone(), portals: self.portals.clone(), geometry: None,
            notes: self.notes.clone(), semantic_volumes: self.semantic_volumes.clone(), volumes: self.volumes.clone(),
            presence: self.presence.clone(), robot_presence: self.robot_presence.clone(), paths: self.paths.clone(),
            paths_product: self.paths_product.clone(),
        }
    }
}
fn limits() -> BufferLimits {
    BufferLimits {
        max_entries: Some(1),
        max_bytes: Some(MAX_BYTES),
        target_duration: None,
    }
}

impl Scene {
    pub fn validate(&self, peer: &str, frame: &str) -> Result<()> {
        ensure!(
            self.schema == SCHEMA
                && self.authority_peer_id == peer
                && self.publication_clock.peer_id == peer,
            "Mobile scene authority/schema mismatch"
        );
        self.portals.validate()?;
        crate::conventions::validate_frame(&self.portals.scenegraph.map.frame)?;
        ensure!(
            self.portals.scenegraph.map.frame.id == frame
                && crate::local_scenes::frame_scene_id(frame)
                    == Some(self.portals.scenegraph.map.map_id.as_str())
                && self.portals.scenegraph.map.domain_reference.is_none(),
            "Mobile scene/profile mismatch"
        );
        self.notes.validate(frame)?;
        if let Some(paths)=&self.paths { paths.validate(frame)?; }
        if let Some(product)=&self.paths_product {
            ensure!(product.peer_id==peer && product.product_id==format!("local-scene:{}:paths",self.portals.scenegraph.map.map_id),"Path Product authority mismatch");
        }
        let mut volume_layer=Notes::empty(frame);
        volume_layer.volumes=self.volumes.clone();
        volume_layer.validate(frame)?;
        if let Some(volumes) = &self.semantic_volumes { volumes.validate(frame)?; }
        ensure!(
            self.presence["frame"] == frame,
            "Presence belongs to another scene"
        );
        if let Some(geometry) = &self.geometry {
            geometry.validate(peer)?;
            ensure!(
                geometry.frame == frame
                    && serde_json::to_value(&geometry.portals)?
                        == serde_json::to_value(&self.portals)?,
                "Geometry belongs to another reference scene"
            );
        }
        ensure!(
            size(self) <= MAX_BYTES,
            "Mobile scene exceeds publication limit"
        );
        Ok(())
    }
    pub fn view(&self) -> Result<Value> {
        let map = &self.portals.scenegraph.map;
        let mut view = if let Some(scene) = &self.geometry {
            crate::scene_mirror::render_scene(scene)?
        } else {
            json!({"frame":map.frame.id,"voxel_size":crate::collaborative_mapper::HOME_VOXEL_SIZE,"voxels":[],"objects":[],"sources":{},"integrated_observations":0,
                "portals":self.portals.scenegraph.anchors.values().collect::<Vec<_>>(),
                "landmarks":crate::landmarks::portal_map(&self.portals,None,&Default::default())?})
        };
        view["notes"] = json!(self.notes.items);
        view["paths"] = json!(self.paths);
        view["paths_product"] = json!(self.paths_product);
        view["volumes"] = json!(self.volumes);
        let mut collection=self.semantic_volumes.clone().unwrap_or_else(|| {
            let mut collection=crate::semantic_volumes::Volumes::empty(&self.portals.scenegraph.map.frame.id);
            collection.items=self.volumes.iter().map(|v|crate::semantic_volumes::Volume::from_mobile(v,None)).collect();
            collection
        });
        for volume in &mut collection.items {
            if let Some(mobile)=self.volumes.iter().find(|v|v.id==volume.id) { volume.semantic_type=mobile.semantic_type.clone(); }
        }
        view["semantic_volumes"] = serde_json::to_value(collection)?;
        crate::mobile_pose::append_labels_in_frame(&mut view, &self.presence, &map.frame.id);
        crate::robot_presence::append_labels(&mut view, &self.robot_presence, &map.frame.id);
        view["environment_name"] = json!(map.name);
        view["frame_description"] = json!(map.frame.origin_description);
        view["local_reference"] = json!(true);
        view["mobile_enabled"] = json!(true);
        view["convention"] = json!("OpenGL · meters");
        view["authority"] = json!("Home · scene-specific peer mapper");
        view["mode"] = json!("Home-owned scenegraph");
        view["origin"] = json!({"policy":"first_observed_portal","portal_id":crate::scene_origin::portal(&self.portals)?,"status":if self.portals.scenegraph.anchors.is_empty(){"waiting_for_first_portal"}else{"established"}});
        view["demo"] = json!(false);
        Ok(view)
    }
    fn mobile(&self) -> Result<Value> {
        let mut value = serde_json::to_value(&self.portals)?;
        value["render"] = self.view()?;
        Ok(value)
    }
}

struct State {
    voxels_cleared: bool,
    map: crate::collaborative_mapper::CollaborativeMap,
    notes: Notes,
    presence: Arc<Mutex<crate::mobile_pose::Presence>>,
    robot_presence: Arc<Mutex<crate::robot_presence::Presence>>,
}
/// Browser writes share the exact live state and persistence path used by SDK peers.
#[derive(Clone)]
pub struct NoteEditor {
    state: Arc<Mutex<State>>,
    frame: String,
    path: std::path::PathBuf,
    reference: Arc<std::sync::RwLock<MapSnapshot>>,
    scene_path: std::path::PathBuf,
}
impl NoteEditor {
    pub(crate) fn integrate_declared(&self, scan: crate::declared_depth::Localized) -> Result<bool> {
        let mut state=self.state.lock().map_err(|_|anyhow!("Scene unavailable"))?;
        state.map.integrate_declared(scan)
    }
    pub fn reset_mapping(&self, expected: &MapSnapshot) -> Result<Value> {
        let _state=self.state.lock().map_err(|_|anyhow!("Scene unavailable"))?;
        let reference=self.reference.read().map_err(|_|anyhow!("Scene reference unavailable"))?;
        ensure!(&*reference==expected,"Scene Portals changed; inspect again before resetting");
        crate::origin_reset::stage(self.scene_path.parent().ok_or_else(||anyhow!("Scene directory missing"))?, expected)
    }
    pub fn reference(&self)->Result<MapSnapshot>{Ok(self.reference.read().map_err(|_|anyhow!("Scene reference unavailable"))?.clone())}
    pub fn clear_voxels(&self) -> Result<Value> {
        let mut state=self.state.lock().map_err(|_|anyhow!("Scene unavailable"))?;
        state.map.clear_voxels()?;
        state.voxels_cleared=true;
        Ok(json!({"tool":"clear_voxels","frame":self.frame,"summary":"Cleared the selected scene's voxels. Notes, volumes and localization are unchanged; active sensors may repopulate the map."}))
    }
    pub fn get_note(&self,id:&str)->Result<crate::notes::TextNote>{self.state.lock().map_err(|_|anyhow!("Scene annotations unavailable"))?.notes.items.iter().find(|n|n.id==id).cloned().ok_or_else(||anyhow!("Note not found in selected scene"))}
    pub fn tool_edit(&self,edit:crate::scene_tools::NoteEdit)->Result<Value>{
        let mut state=self.state.lock().map_err(|_|anyhow!("Scene annotations unavailable"))?;
        let mut notes=state.notes.clone();let receipt=crate::scene_tools::apply_note(&mut notes,edit,&self.frame)?;
        persist_notes(&notes,&self.path)?;state.notes=notes;Ok(receipt)
    }
    pub fn pick(&self,picks:&mut crate::picking::Picks,origin:[f64;3],direction:[f64;3]) -> Result<Value> {
        let state=self.state.lock().map_err(|_|anyhow!("Scene annotations unavailable"))?;
        let voxels=state.map.voxels.as_ref().map(crate::model::occupied).transpose()?.unwrap_or_default();
        picks.pick_frame(origin,direction,&voxels,crate::collaborative_mapper::HOME_VOXEL_SIZE,&self.frame)
    }
    pub fn create(&self, mut note:crate::notes::TextNote) -> Result<crate::notes::TextNote> {
        if let Some(anchor)=&note.anchor_surface { ensure!(anchor.frame==self.frame,"Annotation belongs to another scene"); }
        note.details.prepare_thumbnail()?;
        let mut state=self.state.lock().map_err(|_|anyhow!("Scene annotations unavailable"))?;
        let mut notes=state.notes.clone();notes.items.push(note.clone());notes.validate(&self.frame)?;
        persist_notes(&notes,&self.path)?;state.notes=notes;Ok(note)
    }
    pub fn change(&self, id: &str, edit: Option<(String, Option<crate::notes::NoteDetails>)>) -> Result<Option<crate::notes::TextNote>> {
        let mut state=self.state.lock().map_err(|_|anyhow!("Scene annotations unavailable"))?;
        let action=match &edit { Some((text,_))=>crate::mobile_notes::NoteAction::Edit{id:id.into(),text:text.clone()}, None=>crate::mobile_notes::NoteAction::Delete{id:id.into()} };
        let request=crate::mobile_notes::ChangeNote{request_id:uuid::Uuid::new_v4().to_string(),frame:self.frame.clone(),action};
        let (mut notes,mut result)=crate::mobile_notes::apply_change(&state.notes,&request,&self.frame)?;
        if let Some((_,Some(details)))=edit {
            let note=notes.items.iter_mut().find(|n|n.id==id).ok_or_else(||anyhow!("Annotation not found"))?;
            note.details=details;note.details.prepare_thumbnail()?;
            result.note=Some(note.clone());
        }
        notes.validate(&self.frame)?;
        persist_notes(&notes,&self.path)?;
        state.notes=notes;
        Ok(result.note)
    }
}
pub struct Publisher {
    id: String,
    /// Shared with the ingest operations: a phone-reported Portal replaces it.
    reference: Arc<std::sync::RwLock<MapSnapshot>>,
    state: Arc<Mutex<State>>,
    inbox: crate::mobile_depth::SharedInbox,
    contributions: Arc<Mutex<crate::scene_contribution::Inbox>>,
    scenes: Arc<LocalScenes>,
    output: ConfiguredObservable<Scene>,
    capture: BufferProductCapture<Scene>,
    paths_publisher: crate::scene_paths::Publisher,
    voxel_layers: [crate::voxel_layers::Publisher; 2],
    geometry_output: ConfiguredObservable<CanonicalScene>,
    _geometry_capture: BufferProductCapture<CanonicalScene>,
    occupancy: crate::occupancy_mapper::OccupancyMapper,
    clock: clock::ClockRegistryEntry,
    owner: String,
    started: Instant,
    last_tick: Option<Instant>,
    last_content: Option<Vec<u8>>,
    /// Size and observation count of the voxel map last sent to phones.
    last_checkpoint: Option<(usize, Option<u64>)>,
    last_published: Option<Instant>,
    voxel_tracker: crate::voxel_delta::Tracker,
    voxel_output: ConfiguredObservable<crate::voxel_delta::Update>,
    voxel_capture: BufferProductCapture<crate::voxel_delta::Update>,
    /// Source sequence of the last keyframe, in the Product's Catalog metadata.
    keyframe: Arc<std::sync::atomic::AtomicU64>,
    _components: Vec<Component>,
}
impl Publisher {
    pub fn mount_all(
        runtime: &ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        scenes: Arc<LocalScenes>,
        clock: clock::ClockRegistryEntry,
        started: Instant,
    ) -> Result<Vec<Self>> {
        scenes
            .snapshots
            .iter()
            .filter(|(id, s)| {
                crate::local_scenes::frame_scene_id(&s.scenegraph.map.frame.id) == Some(id.as_str())
            })
            .map(|(id, reference)| {
                Self::mount(
                    runtime,
                    endpoint,
                    scenes.clone(),
                    id,
                    reference.clone(),
                    clock.clone(),
                    started,
                )
            })
            .collect()
    }
    fn mount(
        runtime: &ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        scenes: Arc<LocalScenes>,
        id: &str,
        reference: MapSnapshot,
        clock: clock::ClockRegistryEntry,
        started: Instant,
    ) -> Result<Self> {
        let owner = runtime.peer_id().to_owned();
        let frame = reference.scenegraph.map.frame.id.clone();
        let notes_path = scenes
            .directory
            .join("mobile-notes")
            .join(format!("{id}.json"));
        let notes = if notes_path.exists() {
            ensure!(
                std::fs::metadata(&notes_path)?.len() <= 256 * 1024,
                "Scene notes exceed bounds"
            );
            serde_json::from_slice::<Notes>(&std::fs::read(&notes_path)?)?
        } else {
            Notes::empty(&frame)
        };
        notes.validate(&frame)?;
        scenes.volumes.import_notes(id,&frame,&notes.volumes)?;
        let live_presence = Arc::new(Mutex::new(crate::mobile_pose::Presence::default()));
        let robot_presence = Arc::new(Mutex::new(crate::robot_presence::Presence::default()));
        scenes.phone_presence.lock().map_err(|_| anyhow!("Phone presence unavailable"))?
            .insert(id.into(), live_presence.clone());
        let state = Arc::new(Mutex::new(State {
            voxels_cleared: false,
            map: crate::collaborative_mapper::CollaborativeMap::for_scene(
                &owner,
                &clock.session_id,
                &reference,
            )?,
            notes,
            presence: live_presence.clone(),
            robot_presence: robot_presence.clone(),
        }));
        let inbox = Arc::new(Mutex::new(crate::mobile_depth::Inbox::default()));
        let component = runtime.component(
            ComponentSpec::new(format!("mobile-scene:{id}")).observable(ObservableContract {
                name: "scene".into(),
                datatype: SCHEMA.into(),
                schema: SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }).observable(ObservableContract {
                name: "geometry".into(), datatype: crate::network::SCENE_SCHEMA.into(),
                schema: crate::network::SCENE_SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew], exposure: Exposure::Cluster,
            }).observable(ObservableContract {
                name: "voxels".into(), datatype: crate::voxel_delta::SCHEMA.into(),
                schema: crate::voxel_delta::SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew], exposure: Exposure::Cluster,
            }),
        )?;
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "scene",
                uuid::Uuid::new_v4().to_string(),
                crate::model::clock_ref(&clock),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "scenegraph".into(),
                    datatype: SCHEMA.into(),
                    schema: SCHEMA.into(),
                    observes: "selected_home_scene".into(),
                    unit: Some("meters".into()),
                }),
            )
            .in_registered_frame(crate::conventions::registry(&owner, &frame)?),
        )?;
        let geometry_output = component.configured_observable(ConfiguredObservableSpec::new(
            "geometry", uuid::Uuid::new_v4().to_string(),
            crate::model::clock_ref(&crate::model::clock_entry(&owner, &clock.session_id)),
            PayloadContract::Structured(StructuredPayloadContract {
                modality: "scenegraph".into(), datatype: crate::network::SCENE_SCHEMA.into(),
                schema: crate::network::SCENE_SCHEMA.into(), observes: "accumulated_home_geometry".into(), unit: Some("meters".into()),
            })).in_registered_frame(crate::conventions::registry(&owner, &frame)?))?;
        // Phones' voxels: a keyframe now and then, otherwise only changed cells.
        let voxel_output = component.configured_observable(ConfiguredObservableSpec::new(
            "voxels", uuid::Uuid::new_v4().to_string(), crate::model::clock_ref(&clock),
            PayloadContract::Structured(StructuredPayloadContract {
                modality: "voxels".into(), datatype: crate::voxel_delta::SCHEMA.into(),
                schema: crate::voxel_delta::SCHEMA.into(), observes: "occupied_scene_voxels".into(), unit: Some("meters".into()),
            })).in_registered_frame(crate::conventions::registry(&owner, &frame)?))?;
        component.expose()?;
        let keyframe = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let keyframe_metadata = keyframe.clone();
        // Enough deltas to reach back to the last keyframe, which a new reader starts from.
        let voxel_capture = runtime.capture_buffer_with_metadata(crate::voxel_delta::product_id(id), &voxel_output,
            BufferLimits { max_entries: Some(crate::voxel_delta::KEYFRAME_EVERY as usize + 4), max_bytes: Some(MAX_BYTES), target_duration: None },
            |u: &crate::voxel_delta::Update| 64 + u.set.len() * 15 + u.removed.len() * 12,
            move |o| Ok(Some(CatalogProductMetadata { schema: crate::voxel_delta::METADATA.into(), source_sequence: o.sequence,
                value: json!({"keyframe_source_sequence": keyframe_metadata.load(std::sync::atomic::Ordering::Acquire)}) })))?;
        endpoint.export_product_with_encoder(&voxel_capture.product(), crate::voxel_delta::encoder())?;
        let geometry_capture = runtime.capture_buffer(format!("home-geometry:{id}"), &geometry_output,
            limits(), |s| serde_json::to_vec(s).map_or(usize::MAX, |v| v.len()))?;
        // The retained input is bound locally by the standard SDK mapper. The
        // derived occupancy Product, rather than this extra full scene, is exported.
        let occupancy = crate::occupancy_mapper::OccupancyMapper::mount_scoped(runtime, endpoint,
            &geometry_capture.product(), &owner, crate::model::clock_ref(&clock),
            crate::occupancy_mapper::Config::new(0.), id)?;
        let capture = runtime.capture_buffer(
            crate::local_scenes::mobile_product(id),
            &output,
            limits(),
            size,
        )?;
        endpoint.export_product_with_encoder(&capture.product(), encoder())?;
        let voxel_layers = ["occupied-voxels", "observed-free-voxels"].map(|name|
            crate::voxel_layers::Publisher::mount(runtime, endpoint, id, name,
                crate::model::clock_ref(&clock), crate::conventions::registry(&owner, &frame)?));
        let [occupied_layer, free_layer] = voxel_layers;
        let voxel_layers = [occupied_layer?, free_layer?];
        let mut components = vec![component];
        let contributions = Arc::new(Mutex::new(crate::scene_contribution::Inbox::default()));
        let receiver = runtime.component(ComponentSpec::new(format!("{}:{id}",crate::scene_contribution::COMPONENT)).operable(
            OperableContract{name:"ingest".into(),instruction:crate::scene_contribution::Contribution::DATATYPE.into(),result:"bool".into(),exposure:Exposure::Cluster}))?;
        let reference = Arc::new(std::sync::RwLock::new(reference));
        scenes.note_editors.lock().map_err(|_|anyhow!("Scene annotations unavailable"))?
            .insert(id.into(),NoteEditor{state:state.clone(),frame:frame.clone(),path:notes_path.clone(),
                reference:reference.clone(),scene_path:scenes.directory.join(format!("{id}.json")),
                });
        let queue=contributions.clone();let reference_copy=reference.clone();
        let operation=receiver.operable("ingest",|ctx|!ctx.caller_peer_id.is_empty(),move |ctx,c:crate::scene_contribution::Contribution|{
            crate::home_pose_policy::unsupported("Home posed scene contributions").map_err(rejected)?;
            queue.lock().map_err(|_|InvocationError::TargetUnavailable)?.accept(&ctx.caller_peer_id,c,&*reference_copy.read().map_err(|_|InvocationError::TargetUnavailable)?).map_err(rejected)
        })?;
        receiver.expose()?;endpoint.export_operable(&operation)?;components.push(receiver);

        let depth = runtime.component(
            ComponentSpec::new(format!("{}:{id}", crate::mobile_depth::COMPONENT)).operable(
                OperableContract {
                    name: "ingest".into(),
                    instruction: crate::mobile_depth::Envelope::DATATYPE.into(),
                    result: "bool".into(),
                    exposure: Exposure::Cluster,
                },
            ),
        )?;
        let depth_inbox = inbox.clone();
        let calibration = reference.clone();
        let op = depth.operable(
            "ingest",
            |ctx| !ctx.caller_peer_id.is_empty(),
            move |ctx, e: crate::mobile_depth::Envelope| {
                crate::home_pose_policy::unsupported("Home depth fusion").map_err(rejected)?;
                depth_inbox
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?
                    .accept(
                        &ctx.caller_peer_id,
                        e,
                        &*calibration.read().map_err(|_| InvocationError::TargetUnavailable)?,
                    )
                    .map_err(rejected)
            },
        )?;
        depth.expose()?;
        endpoint.export_operable(&op)?;
        components.push(depth);

        let annotations = runtime.component(
            ComponentSpec::new(format!("{}:{id}", crate::mobile_notes::COMPONENT)).operable(
                OperableContract {
                    name: "place".into(),
                    instruction: crate::mobile_notes::PlaceNote::DATATYPE.into(),
                    result: crate::notes::TextNote::DATATYPE.into(),
                    exposure: Exposure::Cluster,
                },
            ).operable(crate::mobile_notes::change_contract()),
        )?;
        let annotation_state = state.clone();
        let annotation_frame = frame.clone();
        let change_state = state.clone();
        let change_frame = frame.clone();
        let change_path = notes_path.clone();
        let volume_scenes=scenes.clone();
        let volume_key=id.to_owned();
        let memo = std::sync::Mutex::new(crate::mobile_notes::ChangeMemo::default());
        let change = annotations.operable("change", |ctx| !ctx.caller_peer_id.is_empty(),
            move |ctx, request: crate::mobile_notes::ChangeNote| {
                memo.lock().map_err(|_| InvocationError::TargetUnavailable)?.run(&ctx.caller_peer_id, &request, || {
                if matches!(request.action,crate::mobile_notes::NoteAction::PutVolume{..}|crate::mobile_notes::NoteAction::DeleteVolume{..}) {
                    return volume_scenes.volumes.sdk_change(&volume_key,&change_frame,&request);
                }
                let mut state = change_state.lock().map_err(|_| InvocationError::TargetUnavailable)?;
                let (notes, result) = crate::mobile_notes::apply_change(&state.notes, &request, &change_frame)?;
                persist_notes(&notes, &change_path)?;
                state.notes = notes;
                Ok(result)
                }).map_err(rejected)
            })?;
        let op = annotations.operable(
            "place",
            |ctx| !ctx.caller_peer_id.is_empty(),
            move |_, request: crate::mobile_notes::PlaceNote| {
                let mut state = annotation_state
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?;
                place(&mut state, request, &annotation_frame, &notes_path).map_err(rejected)
            },
        )?;
        annotations.expose()?;
        endpoint.export_operable(&op)?;
        endpoint.export_operable(&change)?;
        components.push(annotations);

        let robot_component = runtime.component(ComponentSpec::new(format!("{}:{id}",crate::robot_presence::COMPONENT))
            .operable(OperableContract { name:"update".into(), instruction:crate::robot_presence::Update::DATATYPE.into(),
                result:"bool".into(), exposure:Exposure::Cluster }))?;
        let robot_home = owner.clone();
        let robot_frame = frame.clone();
        let robot_op = robot_component.operable("update", |ctx| !ctx.caller_peer_id.is_empty(),
            move |ctx, update:crate::robot_presence::Update| {
                crate::home_pose_policy::unsupported("Home robot presence").map_err(rejected)?;
                robot_presence.lock().map_err(|_|InvocationError::TargetUnavailable)?
                    .accept(&ctx.caller_peer_id,&robot_home,&robot_frame,update,Instant::now()).map_err(rejected)?;
                Ok(true)
            })?;
        robot_component.expose()?;
        endpoint.export_operable(&robot_op)?;
        components.push(robot_component);
        let presence = runtime.component(
            ComponentSpec::new(format!("{}:{id}", crate::mobile_pose::COMPONENT)).operable(
                OperableContract {
                    name: "update".into(),
                    instruction: crate::mobile_pose::Update::DATATYPE.into(),
                    result: "bool".into(),
                    exposure: Exposure::Cluster,
                },
            ).operable(OperableContract {
                name: "snapshot".into(),
                instruction: crate::mobile_pose::ReadPresence::DATATYPE.into(),
                result: crate::mobile_pose::PresenceSnapshot::DATATYPE.into(),
                exposure: Exposure::Cluster,
            }).operable(OperableContract {
                name: "name".into(),
                instruction: crate::mobile_pose::SetName::DATATYPE.into(),
                result: "bool".into(),
                exposure: Exposure::Cluster,
            }),
        )?;
        // Joystick relay: a phone posts stick values for one robot, and that
        // robot reads its own latest command. Home never moves anything.
        let drive = runtime.component(
            ComponentSpec::new(format!("{}:{id}", crate::mobile_drive::COMPONENT))
                .operable(OperableContract {
                    name: "update".into(),
                    instruction: crate::mobile_drive::Command::DATATYPE.into(),
                    result: "bool".into(),
                    exposure: Exposure::Cluster,
                })
                .operable(OperableContract {
                    name: "snapshot".into(),
                    instruction: crate::mobile_drive::ReadDrive::DATATYPE.into(),
                    result: crate::mobile_drive::Snapshot::DATATYPE.into(),
                    exposure: Exposure::Cluster,
                }),
        )?;
        let board = Arc::new(Mutex::new(crate::mobile_drive::Board::default()));
        let drive_board = board.clone();
        // Accepted commands are also pushed, so a robot need not poll for them.
        let drive_stream = Arc::new(Mutex::new(crate::mobile_drive::Stream::mount(
            runtime,
            endpoint,
            id,
            crate::model::clock_ref(&clock),
            started,
        )?));
        let drive_update = drive.operable(
            "update",
            |ctx| !ctx.caller_peer_id.is_empty(),
            move |ctx, command: crate::mobile_drive::Command| {
                let pushed = crate::mobile_drive::Pushed {
                    driver_peer_id: ctx.caller_peer_id.clone(),
                    target_peer_id: command.target_peer_id.clone(),
                    session: command.session.clone(),
                    sequence: command.sequence,
                    forward: command.forward,
                    turn: command.turn,
                };
                let accepted = drive_board
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?
                    .accept(&ctx.caller_peer_id, command, Instant::now())
                    .map_err(rejected)?;
                if accepted {
                    // The polled snapshot remains the fallback if a push is lost.
                    let _ = drive_stream
                        .lock()
                        .map_err(|_| InvocationError::TargetUnavailable)?
                        .publish(pushed);
                }
                Ok(accepted)
            },
        )?;
        let drive_frame = frame.clone();
        let drive_owner = owner.clone();
        let drive_snapshot = drive.operable(
            "snapshot",
            |ctx| !ctx.caller_peer_id.is_empty(),
            move |ctx, request: crate::mobile_drive::ReadDrive| {
                if request.frame != drive_frame {
                    return Err(InvocationError::Rejected("Drive scene mismatch".into()));
                }
                // A robot can read only the command addressed to itself.
                let command = board
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?
                    .read(&ctx.caller_peer_id, Instant::now());
                Ok(crate::mobile_drive::Snapshot {
                    authority_peer_id: drive_owner.clone(),
                    frame: drive_frame.clone(),
                    command,
                })
            },
        )?;
        drive.expose()?;
        scenes.drives.lock().map_err(|_| anyhow!("Drive relay unavailable"))?
            .insert(id.into(), drive_update.clone());
        endpoint.export_operable(&drive_update)?;
        endpoint.export_operable(&drive_snapshot)?;
        components.push(drive);
        // A phone reports a Portal this scene does not know. Home composes its
        // pose from validated alignment evidence, adds it, and saves the scene.
        let portals = runtime.component(
            ComponentSpec::new(format!("{}:{id}", crate::mobile_portal::COMPONENT)).operable(
                OperableContract {
                    name: "add".into(),
                    instruction: crate::mobile_portal::Report::DATATYPE.into(),
                    result: "bool".into(),
                    exposure: Exposure::Cluster,
                },
            ).operable(OperableContract{name:crate::scene_origin::OPERATION.into(),instruction:crate::scene_origin::Observation::DATATYPE.into(),result:"bool".into(),exposure:Exposure::Cluster}),
        )?;
        let portal_state = state.clone();
        let portal_reference = reference.clone();
        let scene_path = scenes.directory.join(format!("{id}.json"));
        let origin_state=state.clone();let origin_reference=reference.clone();let origin_path=scene_path.clone();
        let origin_owner=owner.clone();let origin_session=clock.session_id.clone();
        let establish=portals.operable(crate::scene_origin::OPERATION,|ctx|!ctx.caller_peer_id.is_empty(),move |_, observation:crate::scene_origin::Observation|{
            // Same writer lock as portal reports: exactly one empty-scene offer wins.
            let mut state=origin_state.lock().map_err(|_|InvocationError::TargetUnavailable)?;
            let mut reference=origin_reference.write().map_err(|_|InvocationError::TargetUnavailable)?;
            let snapshot=crate::scene_origin::establish(&reference,&observation).map_err(rejected)?;
            let map=crate::collaborative_mapper::CollaborativeMap::for_scene(&origin_owner,&origin_session,&snapshot).map_err(rejected)?;
            save_scene(&origin_path,&snapshot).map_err(rejected)?;
            state.map=map;*reference=snapshot;Ok(true)
        })?;
        let add = portals.operable(
            "add",
            |ctx| !ctx.caller_peer_id.is_empty(),
            move |_, report: crate::mobile_portal::Report| {
                let mut state = portal_state
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?;
                let current = portal_reference
                    .read()
                    .map_err(|_| InvocationError::TargetUnavailable)?
                    .clone();
                let anchor = crate::mobile_portal::anchor(&report, &current).map_err(rejected)?;
                let snapshot = state.map.add_portal(anchor).map_err(rejected)?;
                save_scene(&scene_path, &snapshot).map_err(rejected)?;
                *portal_reference
                    .write()
                    .map_err(|_| InvocationError::TargetUnavailable)? = snapshot;
                Ok(true)
            },
        )?;
        portals.expose()?;
        endpoint.export_operable(&establish)?;
        endpoint.export_operable(&add)?;
        components.push(portals);
        // A phone names itself; it applies at once and is saved, with no restart.
        let name_op = presence.operable(
            "name",
            |ctx| !ctx.caller_peer_id.is_empty(),
            |ctx, request: crate::mobile_pose::SetName| {
                crate::mobile_pose::set_name(&ctx.caller_peer_id, &request.name).map_err(rejected)?;
                Ok(true)
            },
        )?;
        let snapshot_state = live_presence.clone();
        let snapshot_frame = frame.clone();
        let snapshot_owner = owner.clone();
        let snapshot_op = presence.operable("snapshot", |ctx| !ctx.caller_peer_id.is_empty(),
            move |_, request: crate::mobile_pose::ReadPresence| {
                if request.frame != snapshot_frame {
                    return Err(InvocationError::Rejected("Presence scene mismatch".into()));
                }
                let presence = snapshot_state.lock().map_err(|_| InvocationError::TargetUnavailable)?
                    .snapshot_in_frame(Instant::now(), &snapshot_frame);
                Ok(crate::mobile_pose::PresenceSnapshot { authority_peer_id: snapshot_owner.clone(), presence })
            })?;
        let presence_state = live_presence;
        let op = presence.operable(
            "update",
            |ctx| !ctx.caller_peer_id.is_empty(),
            move |ctx, update: crate::mobile_pose::Update| {
                crate::home_pose_policy::unsupported("Home phone presence").map_err(rejected)?;
                presence_state
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?
                    .accept_in_frame(&ctx.caller_peer_id, update, Instant::now(), &frame)
                    .map_err(rejected)?;
                Ok(true)
            },
        )?;
        presence.expose()?;
        endpoint.export_operable(&op)?;
        endpoint.export_operable(&snapshot_op)?;
        endpoint.export_operable(&name_op)?;
        components.push(presence);
        let paths_frame=reference.read().map_err(|_|anyhow!("Scene unavailable"))?.scenegraph.map.frame.id.clone();
        let (path_scenes,path_scene,path_frame)=(scenes.clone(),id.to_owned(),paths_frame.clone());
        let paths_publisher=crate::scene_paths::Publisher::mount(runtime,endpoint,id,&owner,&paths_frame,crate::model::clock_ref(&clock),
            move |path,stroke| path_scenes.paths.change(&path_scene,&path_frame,path,stroke))?;
        let (navigation, planner) = crate::scene_navigation::mount(runtime, endpoint, id, &paths_frame, crate::model::clock_ref(&clock),
            NavigationWorld { state: state.clone(), scenes: scenes.clone(), scene: id.into(), frame: paths_frame.clone(), started })?;
        scenes.planners.lock().map_err(|_| anyhow!("Route planner unavailable"))?.insert(id.into(), planner);
        components.push(navigation);
        let mut publisher = Self {
            id: id.into(),
            reference,
            state,
            inbox,
            contributions,
            scenes,
            output,
            capture,
            paths_publisher,
            voxel_layers,
            geometry_output,
            _geometry_capture: geometry_capture,
            occupancy,
            clock,
            owner,
            started,
            last_tick: None,
            last_content: None,
            last_checkpoint: None,
            last_published: None,
            voxel_tracker: Default::default(),
            voxel_output,
            voxel_capture,
            keyframe,
            _components: components,
        };
        publisher.tick()?;
        Ok(publisher)
    }
    pub fn tick(&mut self) -> Result<()> {
        // Paths are lightweight: publish incremental strokes independently of the map's 1 Hz rebuild.
        let frame=self.reference.read().map_err(|_|anyhow!("Scene unavailable"))?.scenegraph.map.frame.id.clone();
        self.paths_publisher.publish(self.started.elapsed().as_nanos() as u64,self.scenes.paths.get(&self.id,&frame)?)?;
        if self
            .last_tick
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return Ok(());
        }
        self.last_tick = Some(Instant::now());
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("Scene unavailable"))?;
        // Serialize draining with resets: never hold a pre-reset batch outside
        // the scene writer lock and integrate it into a freshly reset map.
        let pending = self
            .inbox
            .lock()
            .map_err(|_| anyhow!("Depth inbox unavailable"))?
            .take();
        if state.voxels_cleared {
            self.occupancy.clear_geometry();
            for layer in &self.voxel_layers {layer.clear(self.started.elapsed().as_nanos() as u64)?;}
            state.voxels_cleared=false;
        }
        for contribution in self.contributions.lock().map_err(|_|anyhow!("Contribution inbox unavailable"))?.take() {
            let peer=contribution.source.peer_id.clone();
            if let Err(e)=state.map.integrate_contribution(contribution){state.map.peer_status(&peer,"rejected",&e.to_string());}
        }
        for envelope in pending {
            let peer = envelope.source.peer_id.clone();
            if let Err(e) = state.map.integrate_phone(envelope) {
                state.map.peer_status(&peer, "rejected", &e.to_string());
            }
        }
        let reference = self
            .reference
            .read()
            .map_err(|_| anyhow!("Scene reference unavailable"))?
            .clone();
        let scene = Scene {
            schema: SCHEMA.into(),
            authority_peer_id: self.owner.clone(),
            paths: Some(self.scenes.paths.get(&self.id,&reference.scenegraph.map.frame.id)?),
            paths_product: Some(self.paths_publisher.reference()),
            publication_clock: self.clock.clone(),
            portals: reference.clone(),
            geometry: state.map.scene()?,
            notes: { let mut notes=state.notes.clone();notes.volumes.clear();notes },
            volumes: self.scenes.volumes.mobile(&self.id,&reference.scenegraph.map.frame.id)?,
            semantic_volumes: Some(self.scenes.volumes.get(&self.id, &reference.scenegraph.map.frame.id)?),
            presence: state
                .presence
                .lock().map_err(|_| anyhow!("Phone presence unavailable"))?
                .snapshot_in_frame(Instant::now(), &reference.scenegraph.map.frame.id),
            robot_presence: state.robot_presence.lock().map_err(|_|anyhow!("Robot presence unavailable"))?
                .snapshot(Instant::now(), &reference.scenegraph.map.frame.id),
        };
        scene.validate(&self.owner, &reference.scenegraph.map.frame.id)?;
        self.paths_publisher.publish(self.started.elapsed().as_nanos() as u64,scene.paths.clone().unwrap())?;
        // Phones get voxels as deltas on their own Product, so the scene itself
        // carries no voxel bytes and goes out only when something else changed.
        let light = serde_json::to_vec(&Scene {
            geometry: scene.geometry.as_ref().map(crate::network::CanonicalScene::without_checkpoint),
            ..scene.shallow()
        })?;
        let checkpoint = scene.geometry.as_ref().map(|g| g.voxels.checkpoint.len()).unwrap_or(0);
        let scene_observations = scene.geometry.as_ref().map(|g| g.voxels.integrated_observations);
        let voxels_changed = self.last_checkpoint != Some((checkpoint, scene_observations));
        let rest_changed = self.last_content.as_ref() != Some(&light);
        if !rest_changed && !voxels_changed {
            self.occupancy.tick()?;
            return Ok(());
        }
        let timestamp = self.started.elapsed().as_nanos() as u64;
        if voxels_changed && let Some(geometry) = &scene.geometry {
            for (publisher, layer) in self.voxel_layers.iter().zip(crate::voxel_layers::split(&geometry.voxels)?) {
                publisher.publish(timestamp, layer)?;
            }
            self.geometry_output.publish(timestamp, Arc::new(geometry.clone()))?;
            ensure!(self._geometry_capture.errors().is_empty(), "Home geometry retention failed");
        }
        self.occupancy.tick()?;
        let view = scene.view()?;
        if voxels_changed {
            let size = view["voxel_size"].as_f64().unwrap_or(crate::collaborative_mapper::HOME_VOXEL_SIZE);
            let voxels: Vec<([f64; 3], Option<[f64; 3]>)> = view["voxels"].as_array().map(|list| list.iter().filter_map(|v| Some((
                serde_json::from_value(v["center"].clone()).ok()?,
                serde_json::from_value(v["linear_rgb"].clone()).ok(),
            ))).collect()).unwrap_or_default();
            let frame = view["frame"].as_str().unwrap_or(&frame).to_owned();
            if let Some(update) = self.voxel_tracker.update(&frame, size, &voxels) {
                let keyframe = update.keyframe;
                let (observation, _) = self.voxel_output.publish(timestamp, Arc::new(update))?;
                if keyframe { self.keyframe.store(observation.sequence, std::sync::atomic::Ordering::Release); }
                ensure!(self.voxel_capture.errors().is_empty(), "Phone voxel retention failed");
            }
            self.last_checkpoint = Some((checkpoint, scene_observations));
        }
        self.scenes
            .mobile_views
            .lock()
            .map_err(|_| anyhow!("Scene view unavailable"))?
            .insert(self.id.clone(), view);
        if rest_changed {
            self.output.publish(timestamp, Arc::new(scene))?;
            ensure!(self.capture.errors().is_empty(), "Mobile scene retention failed");
            self.last_content = Some(light);
            self.last_published = Some(Instant::now());
        }
        Ok(())
    }
}
/// The live scene as the route planner reads it: the same map, notes and saved paths peers see.
struct NavigationWorld { state: Arc<Mutex<State>>, scenes: Arc<LocalScenes>, scene: String, frame: String, started: Instant }
impl crate::scene_navigation::World for NavigationWorld {
    fn voxels(&self) -> Result<(Vec<[f64; 3]>, f64)> {
        let state = self.state.lock().map_err(|_| anyhow!("Scene unavailable"))?;
        let voxels = state.map.voxels.as_ref().map(crate::model::occupied).transpose()?.unwrap_or_default();
        Ok((voxels.into_iter().map(|v| v.center).collect(), crate::collaborative_mapper::HOME_VOXEL_SIZE))
    }
    fn note(&self, id: &str) -> Result<Option<(String, [f64; 3])>> {
        let state = self.state.lock().map_err(|_| anyhow!("Scene unavailable"))?;
        Ok(state.notes.items.iter().find(|n| n.id == id).map(|n| (n.text.clone(), n.position)))
    }
    fn store(&self, id: &str, stroke: Option<crate::scene_paths::Stroke>) -> Result<crate::scene_paths::Paths> {
        self.scenes.paths.change(&self.scene, &self.frame, id, stroke)
    }
    fn now(&self) -> u64 { self.started.elapsed().as_nanos() as u64 }
}
/// Replace the saved scene atomically so a restart keeps reported Portals.
fn save_scene(path: &std::path::Path, snapshot: &MapSnapshot) -> Result<()> {
    use std::io::Write;
    snapshot.validate()?;
    let directory = path.parent().ok_or_else(|| anyhow!("Scene path has no directory"))?;
    let temporary = directory.join(format!(".portal.{}.tmp", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(snapshot)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}
fn rejected(e: anyhow::Error) -> InvocationError {
    InvocationError::Rejected(e.to_string())
}

fn place(
    state: &mut State,
    request: crate::mobile_notes::PlaceNote,
    frame: &str,
    path: &std::path::Path,
) -> Result<crate::notes::TextNote> {
    ensure!(
        request.frame == frame,
        "Annotation belongs to another scene"
    );
    uuid::Uuid::parse_str(&request.id)?;
    if let Some(note) = state.notes.items.iter().find(|n| n.id == request.id) {
        ensure!(
            note.text == request.text.trim(),
            "Annotation ID already used"
        );
        return Ok(note.clone());
    }
    let voxels = state
        .map
        .voxels
        .as_ref()
        .map(crate::model::occupied)
        .transpose()?
        .unwrap_or_default();
    let (hit, floor) =
        crate::mobile_notes::intersection(&voxels, crate::collaborative_mapper::HOME_VOXEL_SIZE, request.origin, request.direction)?
            .ok_or_else(|| anyhow!("No floor or voxel within 40 metres"))?;
    let note = crate::notes::TextNote {
        id: request.id,
        text: request.text.trim().into(),
        details: Default::default(),
        position: hit.position,
        anchor_portal: String::new(),
        anchor_robot: None,
        height_m: 0.,
        anchor_surface: Some(crate::notes::SurfaceAnchor {
            frame: frame.into(),
            voxel_center: hit.center,
            beam_diameter_m: 0.,
            floor,
        }),
    };
    let mut notes = state.notes.clone();
    notes.items.push(note.clone());
    notes.validate(frame)?;
    persist_notes(&notes, path)?;
    state.notes = notes;
    Ok(note)
}
fn persist_notes(notes: &Notes, path: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(&notes)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

/// Authenticated SDK subscription, pinned to this scene's Product and frame.
pub async fn receive(
    peer: &auki_sdk::AukiPeer,
    home: &HomeConnection,
    stop: &tokio_util::sync::CancellationToken,
    idle: Duration,
    mut accept: impl FnMut(Value),
) -> Result<()> {
    receive_scene(peer, home, stop, idle, |scene| { accept(scene.mobile()?); Ok(()) }).await
}

/// Consume the authenticated scene without constructing its potentially large
/// render representation. Volume consumers need only the semantic extension.
pub async fn receive_scene(
    peer: &auki_sdk::AukiPeer,
    home: &HomeConnection,
    stop: &tokio_util::sync::CancellationToken,
    idle: Duration,
    mut accept: impl FnMut(&Scene) -> Result<()>,
) -> Result<()> {
    let expected = home.validate(peer)?;
    let client = ComponentProtocolClient::new(peer.protocols());
    let mut failures = Vec::new();
    let mut subscription = None;
    for route in &home.routes {
        let route: auki_sdk::Multiaddr = route.parse()?;
        let attempt = async {
            let CatalogResponse::Snapshot { snapshot } =
                client.catalog_exact(expected, route.clone(), None).await?
            else {
                bail!("No catalog snapshot")
            };
            let entry = snapshot
                .products
                .iter()
                .find(|p| p.manifest.product_id == home.product_id)
                .ok_or_else(|| anyhow!("Selected scene not published by Home"))?;
            ensure!(
                entry.manifest.peer_id == home.peer_id,
                "Scene owner mismatch"
            );
            let stream = client
                .subscribe_product_exact_with_decoder(
                    expected,
                    route,
                    ProductReference {
                        peer_id: home.peer_id.clone(),
                        product_id: home.product_id.clone(),
                        manifest_hash: entry.manifest_hash.clone(),
                    },
                    ObservationStart::LatestExisting,
                    limits(),
                    size,
                    decoder()?,
                )
                .await?;
            Ok::<_, anyhow::Error>(stream)
        };
        let result = tokio::select! { _=stop.cancelled()=>return Ok(()), result=tokio::time::timeout(Duration::from_secs(20),attempt)=>result };
        match result {
            Ok(Ok(s)) => {
                subscription = Some(s);
                break;
            }
            Ok(Err(e)) => failures.push(e.to_string()),
            Err(_) => failures.push("Route timed out".into()),
        }
    }
    let mut subscription =
        subscription.ok_or_else(|| anyhow!("Home connection failed: {}", failures.join("; ")))?;
    let work = async {
        ensure!(subscription.product().producer.payload.schema() == SCHEMA,"Unexpected mobile scene schema");
        let clock = subscription.product().producer.clock.clone();
        let mut received = false;
        loop {
            let next = tokio::select! {
                _=stop.cancelled()=>return Ok(()),
                exit=peer.wait_stopped()=>bail!("Phone peer stopped: {exit:?}"),
                event=tokio::time::timeout(if received {idle} else {Duration::from_secs(30)},subscription.next())=>event,
            };
            match next {
                Err(_) if received => return Ok(()),
                Err(_) => bail!("Home did not publish initial scene"),
                Ok(event) => match event? {
                    Some(RemoteObservationEvent::Observation(o))=>{
                        o.payload.validate(&home.peer_id,&home.frame)?;
                        ensure!(crate::model::clock_ref(&o.payload.publication_clock)==clock,"Scene clock mismatch");
                        accept(&o.payload)?; received=true;
                    },
                    Some(RemoteObservationEvent::Gap(_))=>{},
                    _=>bail!("Scene stream ended"),
                }
            }
        }
    }.await;
    let cleanup = subscription.close().await.map_err(anyhow::Error::from);
    work.and(cleanup)
}

/// Follow the scene's phone voxels: start at Home's last keyframe, then take
/// each delta in order. Returns on stop, or with an error the caller retries.
pub async fn receive_voxels(
    peer: &auki_sdk::AukiPeer,
    home: &HomeConnection,
    stop: &tokio_util::sync::CancellationToken,
    mut accept: impl FnMut(&crate::voxel_delta::Update) -> Result<()>,
) -> Result<()> {
    let scene = home.local_scene_id().ok_or_else(|| anyhow!("Not a local scene"))?;
    let product_id = crate::voxel_delta::product_id(scene);
    let expected = home.validate(peer)?;
    let client = ComponentProtocolClient::new(peer.protocols());
    let mut failures = Vec::new();
    let mut subscription = None;
    for route in &home.routes {
        let route: auki_sdk::Multiaddr = route.parse()?;
        let attempt = async {
            let CatalogResponse::Snapshot { snapshot } = client.catalog_exact(expected, route.clone(), None).await? else {
                bail!("No catalog snapshot")
            };
            let entry = snapshot.products.iter().find(|p| p.manifest.product_id == product_id)
                .ok_or_else(|| anyhow!("Home does not publish phone voxels"))?;
            ensure!(entry.manifest.peer_id == home.peer_id, "Voxel owner mismatch");
            let start = entry.metadata.as_ref().filter(|m| m.schema == crate::voxel_delta::METADATA)
                .and_then(|m| m.value["keyframe_source_sequence"].as_u64()).filter(|s| *s > 0)
                .map_or(ObservationStart::LatestExisting, |sequence| ObservationStart::FromSequence { sequence });
            Ok::<_, anyhow::Error>(client.subscribe_product_exact_with_decoder(expected, route,
                ProductReference { peer_id: home.peer_id.clone(), product_id: product_id.clone(), manifest_hash: entry.manifest_hash.clone() },
                start, BufferLimits::entries(crate::voxel_delta::KEYFRAME_EVERY as usize + 4),
                |u: &crate::voxel_delta::Update| 64 + u.set.len() * 15 + u.removed.len() * 12,
                crate::voxel_delta::decoder()?).await?)
        };
        match tokio::select! { _=stop.cancelled()=>return Ok(()), r=tokio::time::timeout(Duration::from_secs(20),attempt)=>r } {
            Ok(Ok(s)) => { subscription = Some(s); break; }
            Ok(Err(e)) => failures.push(e.to_string()),
            Err(_) => failures.push("Route timed out".into()),
        }
    }
    let mut subscription = subscription.ok_or_else(|| anyhow!("Voxels unavailable: {}", failures.join("; ")))?;
    let work = async {
        loop {
            let next = tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                exit = peer.wait_stopped() => bail!("Phone peer stopped: {exit:?}"),
                event = subscription.next() => event?,
            };
            match next {
                Some(RemoteObservationEvent::Observation(o)) => accept(&o.payload)?,
                // A gap means a missed delta: the Mirror refuses the next one until a keyframe.
                Some(RemoteObservationEvent::Gap(_)) => {}
                _ => bail!("Voxel stream ended"),
            }
        }
    }.await;
    let cleanup = subscription.close().await.map_err(anyhow::Error::from);
    work.and(cleanup)
}
