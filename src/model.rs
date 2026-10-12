use crate::raycast::{Hit, Voxel, raycast};
use anyhow::{Result, anyhow, bail, ensure};
use auki_components::clock::{ClockBody, ClockMeta, ClockRegistryEntry, Scope};
use auki_components::*;
use auki_datatypes::pose::{Quat, SpatialTransform, Vec3};
use auki_geometry::compose_spatial_transforms;
use auki_registry::FrameRegistryEntry;
use auki_scenegraph::{alignment::*, component::*, *};
use auki_voxel_map::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub const FRAME: &str = "domain:b2881524-954d-4979-b715-240ed8f02807:home-origin-UCOF1J40N7O";
pub const VOXEL_SIZE: f64 = 0.1;
pub const SDK_REV: &str = "3342d589605846f5f45c1de5430dd22923555de2";
const MAX_EVENTS: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SensorObservation {
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub clock: ClockReference,
    pub sensor_to_map: RigidTransform,
    pub hit_points_m: Vec<[f64; 3]>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Contribution {
    pub id: String,
    pub peer: String,
    pub scenegraph: Option<MapSnapshot>,
    pub observations: Vec<SensorObservation>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Detection {
    pub id: String,
    pub peer: String,
    pub object_id: String,
    pub label: String,
    pub timestamp_ns: u64,
    pub clock: ClockReference,
    pub image_size: [u32; 2],
    pub pixel: [f64; 2],
    pub intrinsics: [f64; 4], // fx, fy, cx, cy; undistorted source image
    pub camera_to_map: RigidTransform,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "input", rename_all = "snake_case")]
pub enum Event {
    Contribution(Contribution),
    Detection(Detection),
}
impl Event {
    fn key(&self) -> (&str, &str) {
        match self {
            Self::Contribution(c) => (&c.peer, &c.id),
            Self::Detection(d) => (&d.peer, &d.id),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Journal {
    pub schema: u32,
    pub events: Vec<Event>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SemanticObject {
    pub id: String,
    pub label: String,
    pub position: [f64; 3],
    pub source_peer: String,
    pub detection_id: String,
    pub capture_timestamp_ns: u64,
    pub clock: ClockReference,
    pub ray_origin: [f64; 3],
    pub hit: Hit,
}
#[derive(Clone, Debug, Serialize)]
pub struct View {
    pub frame: &'static str,
    pub convention: &'static str,
    pub voxel_size: f64,
    pub voxels: Vec<Voxel>,
    pub portals: Vec<QrAnchor>,
    pub objects: Vec<SemanticObject>,
    pub sources: BTreeMap<String, usize>,
    pub integrated_observations: u64,
    pub sdk_revision: &'static str,
    pub mode: &'static str,
    pub synthetic_layout: Option<crate::synthetic_room::Room>,
    pub demo: bool,
    pub sensor_positions: BTreeMap<String, [f64; 3]>,
}

pub struct Built {
    pub view: View,
    pub portals: MapSnapshot,
    pub voxels: VoxelSnapshot,
    pub catalog: serde_json::Value,
    pub domain_portals: MapSnapshot,
    pub registries: serde_json::Value,
    // Own the actual SDK components for as long as the accepted state is live.
    _map: MapComponent,
    _domain_map: MapComponent,
    _voxel: VoxelMapComponent,
    _runtime: ComponentRuntime,
}

pub struct Store {
    pub journal: Journal,
    pub built: Built,
    pub(crate) path: PathBuf,
    pub(crate) live: Option<crate::runner_voxels::Checkpoint>,
    pub demo: bool,
}
impl Store {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        Self::open_mode(path, true, None, false)
    }
    pub fn open_live(path: impl Into<PathBuf>) -> Result<Self> {
        Self::open_mode(path, false, None, true)
    }
    /// Reference-only graph: never restore a geometry checkpoint into it.
    pub fn open_canonical(path: impl Into<PathBuf>) -> Result<Self> {
        Self::open_mode(path, false, None, false)
    }
    pub fn open_mapper(
        path: impl Into<PathBuf>,
        portals: &MapSnapshot,
        owner: &str,
    ) -> Result<Self> {
        Self::open_mode(path, false, Some((portals, owner)), true)
    }
    fn open_mode(
        path: impl Into<PathBuf>,
        demo: bool,
        mapper: Option<(&MapSnapshot, &str)>,
        restore_geometry: bool,
    ) -> Result<Self> {
        let path = path.into();
        let mut journal: Journal = if path.exists() {
            ensure!(
                fs::metadata(&path)?.len() <= 32 * 1024 * 1024,
                "journal exceeds prototype limit"
            );
            serde_json::from_slice(&fs::read(&path)?)?
        } else {
            Journal {
                schema: 2,
                events: vec![],
            }
        };
        ensure!(
            matches!(journal.schema, 1 | 2) && journal.events.len() <= MAX_EVENTS,
            "unsupported or oversized journal"
        );
        // Explicit user-requested migration: the synthetic scene is authored at
        // the same numeric coordinates in the supplied domain frame. This is not
        // a measured alignment between independently observed physical maps.
        let mut migrated = journal.schema == 1;
        if migrated {
            for event in &mut journal.events {
                match event {
                    Event::Detection(d) if d.camera_to_map.to_frame_id == "home-world" => {
                        d.camera_to_map.to_frame_id = FRAME.into()
                    }
                    Event::Contribution(c) => {
                        for o in &mut c.observations {
                            if o.sensor_to_map.to_frame_id == "home-world" {
                                o.sensor_to_map.to_frame_id = FRAME.into();
                            }
                        }
                    }
                    _ => {}
                }
            }
            journal.schema = 2;
        }
        // Upgrade only the old, reserved demo recipe. Never discard unrelated
        // contributions while replacing an illustrative scene.
        let old_ids = ["iphone-scan-1", "bracketbot-scan-1", "book-detection-1"];
        if journal.events.iter().any(|e| old_ids.contains(&e.key().1)) {
            ensure!(
                journal.events.iter().all(|e| old_ids.contains(&e.key().1)),
                "old demo is mixed with other contributions; keep it in its own journal before replacing"
            );
            let f = fixtures()?;
            let mut replacement = Vec::new();
            for value in f["contributions"].as_array().unwrap() {
                let c: Contribution = serde_json::from_value(value.clone())?;
                if journal
                    .events
                    .iter()
                    .any(|e| matches!(e,Event::Contribution(old) if old.peer==c.peer))
                {
                    replacement.push(Event::Contribution(c));
                }
            }
            if journal
                .events
                .iter()
                .any(|e| matches!(e, Event::Detection(_)))
            {
                replacement.push(Event::Detection(serde_json::from_value(
                    f["detection"].clone(),
                )?));
            }
            journal.events = replacement;
            migrated = true;
        }
        ensure!(
            demo || journal.events.is_empty(),
            "live map requires a separate empty journal; preserve fixture data in its demo journal"
        );
        let built = build_mode(&journal.events, demo)?;
        if migrated {
            persist(&path, &journal)?;
        }
        let mut store = Self {
            journal,
            built,
            path,
            demo,
            live: None,
        };
        if let Some((portals, owner)) = mapper {
            store.configure_mapper(portals, owner)?;
        }
        if restore_geometry {
            store.restore_runner_voxels()?;
        }
        Ok(store)
    }
    fn configure_mapper(&mut self, home: &MapSnapshot, owner: &str) -> Result<()> {
        let converted = crate::conventions::portals(home, crate::conventions::RUNNER_FRAME)?;
        let runtime = ComponentRuntime::new(owner);
        let publication = uuid::Uuid::new_v4().to_string();
        let map = MapComponent::new(
            &runtime,
            MapComponentConfig {
                component_id: "mapper-portals".into(),
                publication_id: publication.clone(),
                clock: clock_ref(&clock_entry(owner, &publication)),
                map: converted.scenegraph.map.clone(),
            },
            tick(),
            |_| true,
            |_| true,
        )?;
        for anchor in converted.scenegraph.anchors.values() {
            insert(&map, anchor.clone())?;
        }
        self.built.portals = snapshot(&map)?;
        let mut definition = self.built.voxels.definition.clone();
        crate::conventions::set_definition_frame(&mut definition, owner, &self.built.portals)?;
        definition.portal_snapshot = map.snapshot_reference();
        self.built.voxels = crate::conventions::voxel_checkpoint(&self.built.voxels, definition)?;
        self.built.view.frame = crate::conventions::RUNNER_FRAME;
        self.built.view.convention = "Z up · right/forward · meters";
        self.built.view.portals = self
            .built
            .portals
            .scenegraph
            .anchors
            .values()
            .cloned()
            .collect();
        self.built.registries = serde_json::json!({"frames":[self.built.voxels.definition.frame],"source_bindings":self.built.voxels.definition.sources,"clocks":[clock_entry(owner,&publication)]});
        self.built.registries["landmarks"] = serde_json::to_value(crate::landmarks::portal_map(
            &self.built.portals,
            Some(map.component().reference().clone()),
            &Default::default(),
        )?)?;
        self.built._map = map;
        self.built._runtime = runtime;
        Ok(())
    }
    pub fn apply(&mut self, event: Event) -> Result<bool> {
        ensure!(self.demo, "fixture writes are disabled for the live map");
        let (peer, id) = event.key();
        ensure!(
            matches!(peer, "iphone" | "bracketbot"),
            "unadmitted test peer"
        );
        valid_text(id)?;
        if let Some(old) = self.journal.events.iter().find(|e| e.key() == event.key()) {
            ensure!(
                old == &event,
                "event identity reused with different content"
            );
            return Ok(false);
        }
        ensure!(
            self.journal.events.len() < MAX_EVENTS,
            "prototype journal is full"
        );
        let mut journal = self.journal.clone();
        journal.events.push(event);
        let mut built = build(&journal.events)?;
        built.registries["text_notes"] = self.built.registries["text_notes"].clone();
        if let Some(volumes) = self.built.registries.get("semantic_volumes") { built.registries["semantic_volumes"] = volumes.clone(); }
        if self.built.registries.get("fiducials").is_some() {
            built.registries["fiducials"] = self.built.registries["fiducials"].clone();
        }

        // Validate and build before persisting; publish only after an atomic replacement.
        persist(&self.path, &journal)?;
        self.journal = journal;
        self.built = built;
        Ok(true)
    }
}
pub(crate) fn persist<T: Serialize>(path: &Path, journal: &T) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".journal-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let bytes = serde_json::to_vec(journal)?;
        ensure!(
            bytes.len() <= 32 * 1024 * 1024,
            "journal exceeds prototype limit"
        );
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
fn valid_text(s: &str) -> Result<()> {
    ensure!(
        !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control),
        "invalid identity or label"
    );
    Ok(())
}
pub fn clock_entry(peer: &str, session: &str) -> ClockRegistryEntry {
    ClockRegistryEntry {
        peer_id: peer.into(),
        session_id: session.into(),
        clock_id: "capture".into(),
        body: ClockBody::MonotonicClock(ClockMeta {
            unit: "nanoseconds".into(),
            monotonic: true,
            epoch: None,
            scope: Scope::DeviceLocal,
        }),
    }
}
pub fn clock_ref(entry: &ClockRegistryEntry) -> ClockReference {
    ClockReference {
        peer_id: entry.peer_id.clone(),
        id: entry.clock_id.clone(),
        hash: entry.hash(),
    }
}
pub fn test_clock(peer: &str) -> ClockReference {
    clock_ref(&clock_entry(peer, "synthetic-v1"))
}
fn tick() -> impl Fn() -> u64 + Send + Sync + 'static {
    let n = AtomicU64::new(1);
    move || n.fetch_add(1, Ordering::SeqCst)
}
fn context() -> InvocationContext {
    InvocationContext {
        invocation_id: uuid::Uuid::new_v4().to_string(),
        caller_peer_id: "home".into(),
        caller_component_id: "canonical-mapper".into(),
    }
}
fn snapshot(map: &MapComponent) -> Result<MapSnapshot> {
    Ok((*map
        .product()
        .latest_existing()?
        .ok_or_else(|| anyhow!("missing Portal snapshot"))?
        .payload)
        .clone())
}
fn insert(map: &MapComponent, anchor: QrAnchor) -> Result<()> {
    InMemoryTransport
        .invoke(
            map.upsert_qr(),
            context(),
            UpsertQr {
                expected_snapshot: map.snapshot_reference(),
                anchor,
            },
        )
        .map_err(|e| anyhow!("{e:?}"))?;
    Ok(())
}
fn map_definition(id: &str, frame: &str) -> MapDefinition {
    MapDefinition {
        map_id: id.into(),
        name: Some(if id == "home" && frame == FRAME { crate::environments::reference_name().into() } else { id.into() }),
        domain_reference: (frame == FRAME).then(|| crate::domain_portals::DOMAIN.into()),
        frame: MapFrame::in_convention(
            frame,
            if frame == FRAME {
                "UCOF1J40N7O center; original OpenGL axes preserved"
            } else {
                "Explicit test origin: Y up, meters"
            },
            CoordinateConvention::OpenGl,
            1.,
        )
        .unwrap(),
    }
}
pub fn portal(frame: &str, id: &str, position: [f64; 3]) -> QrAnchor {
    QrAnchor {
        anchor_id: match id {
            "A" => uuid::Uuid::from_u128(1).to_string(),
            "B" => uuid::Uuid::from_u128(2).to_string(),
            _ => id.into(),
        },
        payload: format!("local:portal:{id}"),
        side_length_m: 0.4,
        pose_in_map: RigidTransform {
            from_frame_id: format!("{id}-frame"),
            to_frame_id: frame.into(),
            translation: position,
            rotation_wxyz: [1., 0., 0., 0.],
        },
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Hits(Vec<[f64; 3]>);
impl ContractType for Hits {
    const DATATYPE: &'static str = "spatial-harness.fixture-hits/v1";
}
struct Source {
    _runtime: ComponentRuntime,
    _component: Component,
    _output: ConfiguredObservable<Hits>,
    capture: BufferProductCapture<Hits>,
}
fn source(peer: &str) -> Result<Source> {
    let runtime = ComponentRuntime::new(peer);
    let component =
        runtime.component(ComponentSpec::new("depth").observable(ObservableContract {
            name: "hits".into(),
            datatype: Hits::DATATYPE.into(),
            schema: Hits::DATATYPE.into(),
            access: vec![ObservationAccess::FollowNew],
            exposure: Exposure::Cluster,
        }))?;
    let output = component.configured_observable(
        ConfiguredObservableSpec::new(
            "hits",
            "synthetic-v1",
            test_clock(peer),
            PayloadContract::Structured(StructuredPayloadContract {
                modality: "depth".into(),
                datatype: Hits::DATATYPE.into(),
                schema: Hits::DATATYPE.into(),
                observes: "test-room".into(),
                unit: Some("meters".into()),
            }),
        )
        .in_registered_frame(FrameRegistryEntry::ros_optical(
            peer,
            format!("{peer}-optical"),
        )),
    )?;
    component.expose()?;
    let capture =
        runtime.capture_buffer("depth-product", &output, BufferLimits::entries(1), |_| {
            32_768
        })?;
    Ok(Source {
        _runtime: runtime,
        _component: component,
        _output: output,
        capture,
    })
}

pub fn build(events: &[Event]) -> Result<Built> {
    build_mode(events, true)
}
fn build_mode(events: &[Event], demo: bool) -> Result<Built> {
    let runtime = ComponentRuntime::new("home");
    let publication = uuid::Uuid::new_v4().to_string();
    let host_clock_definition = clock_entry("home", &publication);
    let host_clock = clock_ref(&host_clock_definition);
    let map = MapComponent::new(
        &runtime,
        MapComponentConfig {
            component_id: "canonical-portals".into(),
            publication_id: publication.clone(),
            clock: host_clock.clone(),
            map: map_definition("home", FRAME),
        },
        tick(),
        |c| c.caller_peer_id == "home",
        |c| c.caller_peer_id == "home",
    )?;
    let supplied_domain = crate::conventions::portals(&crate::domain_portals::load()?, FRAME)?;
    for anchor in supplied_domain.scenegraph.anchors.values() {
        insert(&map, anchor.clone())?;
    }
    if demo {
        insert(&map, portal(FRAME, "A", [-1.2, 0.65, -0.49]))?;
    }
    let mut aligned = Vec::new();
    let mut seen_observations: BTreeMap<(String, u64), SensorObservation> = BTreeMap::new();
    let mut source_counts = BTreeMap::from([("iphone".into(), 0), ("bracketbot".into(), 0)]);
    for event in events {
        let Event::Contribution(c) = event else {
            continue;
        };
        ensure!(
            matches!(c.peer.as_str(), "iphone" | "bracketbot"),
            "unadmitted test peer"
        );
        valid_text(&c.id)?;
        ensure!(
            c.observations.len() <= 16,
            "too many observations in one contribution"
        );
        ensure!(
            c.scenegraph.is_some() || !c.observations.is_empty(),
            "empty contribution"
        );
        let alignment = if let Some(remote) = &c.scenegraph {
            remote.validate()?;
            ensure!(
                remote.scenegraph.map.frame.convention == Some(CoordinateConvention::OpenGl)
                    && remote.scenegraph.map.frame.meters_per_unit == 1.,
                "first slice requires explicit OpenGL meters"
            );
            ensure!(
                remote.scenegraph.map.frame.id != FRAME,
                "peer scenegraph needs a distinct frame identity"
            );
            let current = snapshot(&map)?;
            let mut checker = MapAlignmentChecker::new(AlignmentOptions::default())?;
            checker.receive_snapshot("home", map.snapshot_reference(), current.clone())?;
            checker.receive_snapshot(
                "remote",
                SnapshotReference {
                    product: ProductReference {
                        peer_id: c.peer.clone(),
                        product_id: format!("fixture-map/{}", c.id),
                        manifest_hash: "local-fixture-envelope".into(),
                    },
                    sequence: 1,
                },
                remote.clone(),
            )?;
            let transform = match checker.check("remote", "home") {
                AlignmentResult::Available { transform, .. } => transform,
                other => bail!("scenegraph alignment unavailable: {other:?}"),
            };
            for anchor in remote.scenegraph.anchors.values() {
                let mut a = anchor.clone();
                a.pose_in_map = compose(&a.pose_in_map, &transform)?;
                if let Some(existing) = current.scenegraph.anchors.get(&a.anchor_id) {
                    ensure!(
                        same_anchor(existing, &a),
                        "conflicting existing Portal; corrections require an explicit policy"
                    );
                } else {
                    insert(&map, a)?;
                }
            }
            Some(transform)
        } else {
            None
        };
        for o in &c.observations {
            ensure!(
                o.clock == test_clock(&c.peer),
                "capture clock not bound to this test peer/session"
            );
            let mut o = o.clone();
            if o.sensor_to_map.to_frame_id != FRAME {
                let transform = alignment.as_ref().ok_or_else(|| {
                    anyhow!(
                        "observation needs an aligned scenegraph or an explicit home-frame pose"
                    )
                })?;
                o.sensor_to_map = compose(&o.sensor_to_map, transform)?;
            }
            let key = (c.peer.clone(), o.sequence);
            if let Some(old) = seen_observations.get(&key) {
                ensure!(
                    old == &o,
                    "source observation identity reused with different content"
                );
                continue;
            }
            seen_observations.insert(key, o.clone());
            aligned.push((c.peer.clone(), o));
        }
    }
    let portals = snapshot(&map)?;
    let sources = [source("iphone")?, source("bracketbot")?];
    let first = sources[0].capture.product().reference();
    let mut definition = VoxelMapDefinition::from_portal_map(
        "home",
        "home-voxels",
        map.snapshot_reference(),
        &portals,
        FrameRegistryEntry::opengl("home", FRAME),
        first,
        FrameRegistryEntry::ros_optical("iphone", "iphone-optical"),
        test_clock("iphone"),
        VOXEL_SIZE,
    )?;
    definition.sources.push(SensorBinding {
        sensor_product: sources[1].capture.product().reference(),
        sensor_frame: FrameRegistryEntry::ros_optical("bracketbot", "bracketbot-optical"),
        observation_clock: test_clock("bracketbot"),
    });
    let mut voxel = VoxelMapComponent::new(
        &runtime,
        "canonical-voxels",
        &format!("{publication}-voxels"),
        host_clock.clone(),
        definition,
        tick(),
    )?;
    // SDK rejects invalid geometry, stale ordering, clocks and poses. Exact latest
    // replays are no-ops even when carried inside a different contribution envelope.
    let mut sensor_positions = BTreeMap::new();
    for (peer, o) in aligned {
        sensor_positions.insert(peer.clone(), o.sensor_to_map.translation);
        let index = usize::from(peer == "bracketbot");
        let result = voxel.integrate(DepthObservation {
            source: sources[index].capture.product().reference(),
            sequence: o.sequence,
            timestamp_ns: o.timestamp_ns,
            clock: o.clock.clone(),
            sensor_frame_id: format!("{peer}-optical"),
            hit_points_m: o.hit_points_m,
            pose: TimedSensorPose {
                timestamp_ns: o.timestamp_ns,
                clock: o.clock,
                sensor_to_map: o.sensor_to_map,
                portal_snapshot: map.snapshot_reference(),
            },
        })?;
        if result == IntegrationResult::Applied {
            *source_counts.get_mut(&peer).unwrap() += 1;
        }
    }
    let voxels = (*voxel
        .product()
        .latest_existing()?
        .ok_or_else(|| anyhow!("missing voxels"))?
        .payload)
        .clone();
    let cells = occupied(&voxels)?;
    let mut objects: BTreeMap<String, SemanticObject> = BTreeMap::new();
    for (index, event) in events.iter().enumerate() {
        if let Event::Detection(d) = event {
            // Use the map that existed when the detection was accepted, not later
            // geometry. Store replay remains deterministic as new scans arrive.
            let historical = if events[index + 1..]
                .iter()
                .any(|e| matches!(e, Event::Contribution(_)))
            {
                build_geometry_prefix(&events[..index])?
            } else {
                cells.clone()
            };
            let object = project(d, &historical)?;
            if let Some(old) = objects.get(&object.id) {
                ensure!(old == &object, "object identity conflict");
            }
            objects.insert(object.id.clone(), object);
        }
    }
    let view = View {
        frame: FRAME,
        convention: "OpenGL · Y up · meters",
        voxel_size: VOXEL_SIZE,
        voxels: cells,
        portals: portals.scenegraph.anchors.values().cloned().collect(),
        objects: objects.into_values().collect(),
        sources: source_counts,
        integrated_observations: voxels.integrated_observations,
        sdk_revision: SDK_REV,
        mode: if demo {
            "Illustrative workspace · simulated scans"
        } else {
            "Live runner mapping"
        },
        demo,
        synthetic_layout: if demo {
            Some(crate::synthetic_room::Room::load()?)
        } else {
            None
        },
        sensor_positions,
    };
    let domain_snapshot = crate::domain_portals::load()?;
    let domain_map = MapComponent::new(
        &runtime,
        MapComponentConfig {
            component_id: "domain-portals".into(),
            publication_id: format!("{publication}-domain"),
            clock: host_clock,
            map: domain_snapshot.scenegraph.map.clone(),
        },
        tick(),
        |c| c.caller_peer_id == "home",
        |c| c.caller_peer_id == "home",
    )?;
    for anchor in domain_snapshot.scenegraph.anchors.values() {
        insert(&domain_map, anchor.clone())?;
    }
    let domain_portals = snapshot(&domain_map)?;
    let catalog = serde_json::to_value(runtime.catalog().snapshot())?;
    let mut registries = serde_json::json!({
        "clocks": [host_clock_definition, clock_entry("iphone", "synthetic-v1"), clock_entry("bracketbot", "synthetic-v1")],
        "frames": [FrameRegistryEntry::opengl("home", FRAME), FrameRegistryEntry::opengl("home", crate::domain_portals::FRAME), FrameRegistryEntry::ros_optical("iphone", "iphone-optical"), FrameRegistryEntry::ros_optical("bracketbot", "bracketbot-optical")],
        "source_catalogs": sources.iter().map(|s| s._runtime.catalog().snapshot()).collect::<Vec<_>>()
    });
    registries["landmarks"] = serde_json::to_value(crate::landmarks::portal_map(
        &portals,
        Some(map.component().reference().clone()),
        &Default::default(),
    )?)?;
    Ok(Built {
        registries,
        domain_portals,
        _domain_map: domain_map,
        view,
        portals,
        voxels,
        catalog,
        _map: map,
        _voxel: voxel,
        _runtime: runtime,
    })
}
fn build_geometry_prefix(events: &[Event]) -> Result<Vec<Voxel>> {
    let contributions: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::Contribution(_)))
        .cloned()
        .collect();
    Ok(build(&contributions)?.view.voxels)
}
pub(crate) fn occupied(snapshot: &VoxelSnapshot) -> Result<Vec<Voxel>> {
    Ok(snapshot
        .accumulator()?
        .viewer_snapshot(0.)?
        .chunks
        .into_iter()
        .flat_map(|c| c.voxels)
        .filter(|v| v.occupancy_evidence > 0.)
        .map(|v| Voxel {
            center: std::array::from_fn(|i| v.center_m[i] + snapshot.definition.grid_origin_m[i]),
            evidence: v.occupancy_evidence,
            linear_rgb: v.linear_rgb,
        })
        .collect())
}
pub(crate) fn same_anchor(a: &QrAnchor, b: &QrAnchor) -> bool {
    a.anchor_id == b.anchor_id
        && a.payload == b.payload
        && (a.side_length_m - b.side_length_m).abs() < 1e-9
        && a.pose_in_map.from_frame_id == b.pose_in_map.from_frame_id
        && a.pose_in_map.to_frame_id == b.pose_in_map.to_frame_id
        && a.pose_in_map
            .translation
            .iter()
            .zip(b.pose_in_map.translation)
            .all(|(x, y)| (x - y).abs() < 1e-6)
        && {
            let dot: f64 = a
                .pose_in_map
                .rotation_wxyz
                .iter()
                .zip(b.pose_in_map.rotation_wxyz)
                .map(|(x, y)| x * y)
                .sum();
            (dot.abs() - 1.).abs() < 1e-6
        }
}
fn numeric(t: &RigidTransform) -> SpatialTransform {
    let [x, y, z] = t.translation;
    let [w, qx, qy, qz] = t.rotation_wxyz;
    SpatialTransform {
        translation: Some(Vec3 { x, y, z }),
        orientation: Some(Quat {
            x: qx,
            y: qy,
            z: qz,
            w,
        }),
    }
}
fn compose(a: &RigidTransform, b: &RigidTransform) -> Result<RigidTransform> {
    ensure!(
        a.to_frame_id == b.from_frame_id,
        "disconnected transform endpoints"
    );
    let t = compose_spatial_transforms(&numeric(a), &numeric(b)).map_err(|e| anyhow!("{e:?}"))?;
    let p = t.translation.unwrap();
    let q = t.orientation.unwrap();
    Ok(RigidTransform {
        from_frame_id: a.from_frame_id.clone(),
        to_frame_id: b.to_frame_id.clone(),
        translation: [p.x, p.y, p.z],
        rotation_wxyz: [q.w, q.x, q.y, q.z],
    })
}
fn rotated(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let [w, x, y, z] = q;
    let cross = [
        y * v[2] - z * v[1],
        z * v[0] - x * v[2],
        x * v[1] - y * v[0],
    ];
    let cross2 = [
        y * cross[2] - z * cross[1],
        z * cross[0] - x * cross[2],
        x * cross[1] - y * cross[0],
    ];
    std::array::from_fn(|i| v[i] + 2. * (w * cross[i] + cross2[i]))
}
fn project(d: &Detection, voxels: &[Voxel]) -> Result<SemanticObject> {
    ensure!(
        matches!(d.peer.as_str(), "iphone" | "bracketbot") && d.clock == test_clock(&d.peer),
        "unbound detection source or clock"
    );
    valid_text(&d.id)?;
    valid_text(&d.object_id)?;
    valid_text(&d.label)?;
    ensure!(
        d.timestamp_ns > 0
            && d.camera_to_map.from_frame_id == format!("{}-optical", d.peer)
            && d.camera_to_map.to_frame_id == FRAME,
        "invalid detection pose provenance"
    );
    let [fx, fy, cx, cy] = d.intrinsics;
    ensure!(
        d.intrinsics
            .iter()
            .chain(d.pixel.iter())
            .all(|x| x.is_finite())
            && fx > 0.
            && fy > 0.,
        "invalid calibration"
    );
    ensure!(
        d.image_size.iter().all(|v| *v > 0)
            && d.pixel[0] >= 0.
            && d.pixel[1] >= 0.
            && d.pixel[0] < f64::from(d.image_size[0])
            && d.pixel[1] < f64::from(d.image_size[1]),
        "pixel outside source image"
    );
    let q = d.camera_to_map.rotation_wxyz;
    ensure!(
        q.iter().all(|v| v.is_finite()) && (q.iter().map(|v| v * v).sum::<f64>() - 1.).abs() < 1e-6,
        "invalid camera rotation"
    );
    let direction = rotated(q, [(d.pixel[0] - cx) / fx, (d.pixel[1] - cy) / fy, 1.]);
    let hit = raycast(
        voxels,
        VOXEL_SIZE,
        d.camera_to_map.translation,
        direction,
        100.,
    )?
    .ok_or_else(|| anyhow!("no occupied voxel hit; detection remains unplaced"))?;
    Ok(SemanticObject {
        id: d.object_id.clone(),
        label: d.label.clone(),
        position: hit.position,
        source_peer: d.peer.clone(),
        detection_id: d.id.clone(),
        capture_timestamp_ns: d.timestamp_ns,
        clock: d.clock.clone(),
        ray_origin: d.camera_to_map.translation,
        hit,
    })
}

/// Deliberately synthetic scene and camera data; no YOLO inference or device I/O.
pub fn fixtures() -> Result<serde_json::Value> {
    let mut contributions = Vec::new();
    for (index, peer) in ["iphone", "bracketbot"].iter().enumerate() {
        let frame = format!("{peer}-world");
        let offset = if index == 0 {
            [0., 0., 0.]
        } else {
            [0.5, 0., 0.3]
        };
        let mut scene = Scenegraph::new(map_definition(&format!("{peer}-map"), &frame))?;
        let mut a = portal(&frame, "A", [-1.2, 0.65, -0.49]);
        for (i, v) in offset.iter().enumerate() {
            a.pose_in_map.translation[i] -= v;
        }
        scene.anchors.insert(a.anchor_id.clone(), a);
        let room = crate::synthetic_room::Room::load()?;
        let mut observations = Vec::new();
        let mut tile = 0;
        for tx in (-122..22).step_by(20) {
            for tz in (-7..69).step_by(20) {
                let camera = [
                    f64::from(tx + 10) * 0.1,
                    if index == 0 { 1.8 } else { 1.15 },
                    f64::from(tz + 10) * 0.1,
                ];
                let mut points = Vec::new();
                for x in tx..(tx + 20).min(22) {
                    for z in tz..(tz + 20).min(69) {
                        let wx = f64::from(x) * 0.1 + 0.05;
                        let wz = f64::from(z) * 0.1 + 0.05;
                        if !room.contains_floor(wx, wz) {
                            continue;
                        }
                        let hit = room.hit(camera, [wx, -0.05, wz]);
                        points.push([hit[0] - camera[0], hit[2] - camera[2], camera[1] - hit[1]]);
                    }
                }
                if points.is_empty() {
                    continue;
                }
                let owner = tile % 2;
                tile += 1;
                if owner != index {
                    continue;
                }
                let sequence = observations.len() as u64 + 1;
                observations.push(SensorObservation {
                    sequence,
                    timestamp_ns: sequence * if index == 0 { 1000 } else { 10 },
                    clock: test_clock(peer),
                    sensor_to_map: RigidTransform {
                        from_frame_id: format!("{peer}-optical"),
                        to_frame_id: frame.clone(),
                        translation: std::array::from_fn(|i| camera[i] - offset[i]),
                        rotation_wxyz: [
                            std::f64::consts::FRAC_1_SQRT_2,
                            std::f64::consts::FRAC_1_SQRT_2,
                            0.,
                            0.,
                        ],
                    },
                    hit_points_m: points,
                });
            }
        }
        if index == 0 {
            let camera = [-3.65, 1.8, 2.15];
            let mut points = Vec::new();
            for x in -46..-28 {
                for z in 16..26 {
                    let hit = room.hit(
                        camera,
                        [f64::from(x) * 0.1 + 0.05, -0.05, f64::from(z) * 0.1 + 0.05],
                    );
                    points.push([hit[0] - camera[0], hit[2] - camera[2], camera[1] - hit[1]]);
                }
            }
            let hit = room.hit(camera, [-3.65, -0.05, 2.15]);
            points.push([hit[0] - camera[0], hit[2] - camera[2], camera[1] - hit[1]]);
            let sequence = observations.len() as u64 + 1;
            observations.push(SensorObservation {
                sequence,
                timestamp_ns: sequence * 1000,
                clock: test_clock(peer),
                sensor_to_map: RigidTransform {
                    from_frame_id: format!("{peer}-optical"),
                    to_frame_id: frame.clone(),
                    translation: camera,
                    rotation_wxyz: [
                        std::f64::consts::FRAC_1_SQRT_2,
                        std::f64::consts::FRAC_1_SQRT_2,
                        0.,
                        0.,
                    ],
                },
                hit_points_m: points,
            });
        }
        contributions.push(Contribution {
            id: format!("{peer}-room-v2"),
            peer: (*peer).into(),
            scenegraph: Some(MapSnapshot::new(scene)?),
            observations,
        });
    }
    let detection = Detection {
        id: "book-room-v2".into(),
        peer: "iphone".into(),
        object_id: "book-1".into(),
        label: "Book".into(),
        timestamp_ns: contributions[0].observations.last().unwrap().timestamp_ns,
        clock: test_clock("iphone"),
        image_size: [640, 480],
        pixel: [320., 240.],
        intrinsics: [500., 500., 320., 240.],
        camera_to_map: RigidTransform {
            from_frame_id: "iphone-optical".into(),
            to_frame_id: FRAME.into(),
            translation: [-3.65, 1.8, 2.15],
            rotation_wxyz: [
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
                0.,
                0.,
            ],
        },
    };
    Ok(
        serde_json::json!({"contributions":contributions,"detection":detection,"clocks":[clock_entry("iphone","synthetic-v1"),clock_entry("bracketbot","synthetic-v1")]}),
    )
}
