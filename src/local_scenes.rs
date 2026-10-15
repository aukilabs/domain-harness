//! Persisted, independent local SDK scenegraphs. Selecting one never changes
//! the legacy canonical mapper's calibration or journal.
use anyhow::{Result, anyhow, ensure};
use auki_components::{ComponentRuntime, InMemoryTransport, InvocationContext};
use auki_scenegraph::{
    CoordinateConvention, MapDefinition, MapFrame, MapSnapshot,
    Scenegraph,
    component::{MapComponent, MapComponentConfig, UpsertQr},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write, path::Path, time::Instant};

pub struct LocalScenes {
    pub reference_enabled: bool,
    pub note_editors: std::sync::Mutex<BTreeMap<String, crate::local_mobile::NoteEditor>>,
    pub volumes: crate::semantic_volumes::Storage,
    pub paths: crate::scene_paths::Storage,
    pub snapshots: BTreeMap<String, MapSnapshot>,
    pub(crate) directory: std::path::PathBuf,
    pub(crate) mobile_views: std::sync::Mutex<BTreeMap<String, Value>>,
    pub(crate) phone_presence: std::sync::Mutex<BTreeMap<String, std::sync::Arc<std::sync::Mutex<crate::mobile_pose::Presence>>>>,
    /// Each scene's joystick relay, so Home's own viewer drives through the
    /// same board, single-driver rule and push stream as a phone.
    pub(crate) drives: std::sync::Mutex<BTreeMap<String, auki_components::Operable<crate::mobile_drive::Command, bool>>>,
    /// Each scene's route planner, so Home can plan a robot's way as its own peer.
    pub(crate) planners: std::sync::Mutex<BTreeMap<String, auki_components::Operable<crate::scene_navigation::RouteRequest, crate::scene_navigation::Route>>>,
    /// Registry-declared robots per scene frame, kept by `registry_robots::serve`.
    pub registry_robots: crate::registry_robots::Entries,
}

/// Instance-local lifecycle choices. Built-in templates must not resurrect a
/// deleted scene. Read only at startup, before mounting any scene Products.
#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    #[serde(default)]
    delete_reference: bool,
    #[serde(default)]
    deleted: std::collections::BTreeSet<String>,
    #[serde(default)]
    names: BTreeMap<String, String>,
}
impl Settings {
    fn load(directory: &Path) -> Result<Self> {
        let path = directory.join(".settings/catalog.json");
        if !path.exists() { return Ok(Self::default()); }
        ensure!(std::fs::metadata(&path)?.len() <= 32 * 1024, "Scene settings exceed bounds");
        let settings: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(settings.deleted.len() <= 128 && settings.names.len() <= 128, "Too many scene settings");
        ensure!(settings.deleted.iter().all(|id| valid_id(id)), "Invalid deleted scene ID");
        for (id, name) in &settings.names {
            ensure!(valid_id(id) && !settings.deleted.contains(id), "Invalid renamed scene ID");
            ensure!(!name.trim().is_empty() && name.len() <= 120 && !name.chars().any(char::is_control), "Invalid scene name");
        }
        Ok(settings)
    }

    fn archive_deleted(&self, directory: &Path) -> Result<()> {
        for id in &self.deleted {
            // Exact scene-owned targets only; never archive Home's identity or
            // the enclosing data directory. A completed move is restart-safe.
            for relative in [format!("{id}.json"), format!("mobile-notes/{id}.json"),
                format!("semantic-volumes/{id}.json"), format!("paths/{id}.json"),
                format!(".origin-resets/{id}.pending")] {
                let from = directory.join(&relative);
                if !from.exists() { continue; }
                let to = directory.join(".deleted").join(&relative);
                ensure!(!to.exists(), "Deleted scene backup already exists: {relative}");
                std::fs::create_dir_all(to.parent().unwrap())?;
                std::fs::rename(from, &to)?;
                std::fs::File::open(to.parent().unwrap())?.sync_all()?;
            }
        }
        Ok(())
    }
}

/// Stable physical-origin identity, not a coordinate-convention alias.
pub fn frame_scene_id(frame: &str) -> Option<&str> {
    if let Some((id,epoch))=frame.strip_prefix("local-scene:").and_then(|s|s.split_once(":epoch-")) {
        return (valid_id(id) && uuid::Uuid::parse_str(epoch).is_ok()).then_some(id);
    }
    if let Some(id)=frame.strip_prefix("local-scene:").and_then(|s|s.strip_suffix(":origin")) {return valid_id(id).then_some(id);}
    let (id, portal) = frame.strip_prefix("local-scene:")?.split_once(":portal-")?;
    (valid_id(id)
        && portal.len() == 11
        && portal
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
    .then_some(id)
}

pub fn mobile_product(id: &str) -> String {
    format!("local-scene:{id}:mobile")
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

impl LocalScenes {
    /// Plan a route in `id` as `caller` (Home's own peer), through the scene's
    /// `plan` Operable: the same planner, map and shared-route rules as any peer.
    pub fn plan_route(&self, id: &str, caller: &str, request: crate::scene_navigation::RouteRequest) -> Result<crate::scene_navigation::Route> {
        let planner = self.planners.lock().map_err(|_| anyhow!("Route planner unavailable"))?
            .get(id).cloned().ok_or_else(|| anyhow!("This scene has no route planner"))?;
        let context = auki_components::InvocationContext {
            invocation_id: uuid::Uuid::new_v4().to_string(),
            caller_peer_id: caller.into(),
            caller_component_id: "home-navigation".into(),
        };
        Ok(planner.invoke(context, request).map_err(|e| anyhow!("{e}"))?.result)
    }
    /// Send one stick command for `id`'s relay as `driver` (Home's own peer).
    /// The robot, not Home, decides whether this driver may move it.
    pub fn drive(&self, id: &str, driver: &str, command: crate::mobile_drive::Command) -> Result<bool> {
        let relay = self.drives.lock().map_err(|_| anyhow!("Drive relay unavailable"))?
            .get(id).cloned().ok_or_else(|| anyhow!("This scene has no drive relay"))?;
        let context = auki_components::InvocationContext {
            invocation_id: uuid::Uuid::new_v4().to_string(),
            caller_peer_id: driver.into(),
            caller_component_id: "home-viewer".into(),
        };
        Ok(relay.invoke(context, command).map_err(|e| anyhow!("{e}"))?.result)
    }
    pub fn load(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        let settings = Settings::load(directory)?;
        settings.archive_deleted(directory)?;
        crate::origin_reset::apply_pending(directory)?;
        // Independent local scenes start uncalibrated. Historical origin entries
        // identify configured scenes, but their short codes are never seeded.
        for env in &crate::environments::CATALOG.environments {
            if settings.deleted.contains(&env.id) { continue; }
            let Some(_legacy_origin) = &env.origin else {
                continue;
            };
            ensure!(valid_id(&env.id), "Invalid local scene ID");
            let path = directory.join(format!("{}.json", env.id));
            if path.exists() {
                continue;
            } // Existing saved scenes are authoritative.
            let frame = format!("local-scene:{}:origin", env.id);
            let graph = Scenegraph::new(MapDefinition {
                map_id: env.id.clone(),
                name: Some(env.name.clone()),
                domain_reference: None,
                frame: MapFrame::in_convention(
                    &frame,
                    crate::scene_origin::DESCRIPTION,
                    CoordinateConvention::OpenGl,
                    1.,
                )?,
            })?;
            let snapshot = MapSnapshot::new(graph)?;
            let temporary = directory.join(format!(".{}.{}.tmp", env.id, uuid::Uuid::new_v4()));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(&snapshot)?)?;
            file.sync_all()?;
            std::fs::rename(temporary, path)?;
        }
        let mut snapshots = BTreeMap::new();
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_file()
                || entry.path().extension().and_then(|v| v.to_str()) != Some("json")
            {
                continue;
            }
            ensure!(
                entry.metadata()?.len() <= 8 * 1024 * 1024 && snapshots.len() < 128,
                "Local scene collection exceeds bounds"
            );
            let mut snapshot: MapSnapshot = serde_json::from_slice(&std::fs::read(entry.path())?)?;
            snapshot.validate()?;
            if settings.deleted.contains(&snapshot.scenegraph.map.map_id) { continue; }
            if let Some(name) = settings.names.get(&snapshot.scenegraph.map.map_id) {
                if snapshot.scenegraph.map.name.as_ref() != Some(name) {
                    snapshot.scenegraph.map.name = Some(name.clone());
                    // Rebuild USD too; editing the JSON name alone invalidates
                    // the SDK snapshot. Frame identity and anchors are unchanged.
                    snapshot = MapSnapshot::new(snapshot.scenegraph)?;
                    crate::model::persist(&entry.path(), &snapshot)?;
                }
            }
            let id = &snapshot.scenegraph.map.map_id;
            ensure!(valid_id(id), "Invalid saved scene ID");
            ensure!(
                snapshot.scenegraph.map.frame.convention == Some(CoordinateConvention::OpenGl)
                    && snapshot.scenegraph.map.frame.meters_per_unit == 1.,
                "Local scene viewer requires OpenGL meters"
            );
            ensure!(
                snapshots.insert(id.clone(), snapshot).is_none(),
                "Duplicate local scene ID"
            );
        }
        Ok(Self {
            reference_enabled: !settings.delete_reference,
            note_editors: Default::default(),
            volumes: crate::semantic_volumes::Storage::new(directory.join("semantic-volumes")),
            paths: crate::scene_paths::Storage::new(directory.join("paths")),
            snapshots,
            directory: directory.into(),
            mobile_views: Default::default(),
            phone_presence: Default::default(),
            drives: Default::default(),
            planners: Default::default(),
            registry_robots: Default::default(),
        })
    }
    /// Read live presence independently of the 1 Hz voxel/scene publication.
    pub fn presence(&self, id: &str) -> Result<Value> {
        let frame = &self.snapshot(id)?.scenegraph.map.frame.id;
        let entries = self.phone_presence.lock().map_err(|_| anyhow!("Phone presence unavailable"))?;
        match entries.get(id) {
            Some(presence) => Ok(presence.lock().map_err(|_| anyhow!("Phone presence unavailable"))?
                .snapshot_in_frame(Instant::now(), frame)),
            None => Ok(json!({"frame":frame,"poses":[]})),
        }
    }

    pub fn rows(&self) -> Vec<Value> {
        self.snapshots.iter().map(|(id,s)|json!({"id":format!("local:{id}"),"name":s.scenegraph.map.name.as_deref().unwrap_or(id),"available":true})).collect()
    }
    pub fn preferred_id(&self) -> Option<String> {
        crate::environments::CATALOG
            .preferred_scene
            .as_ref()
            .filter(|id| self.snapshots.contains_key(*id))
            .map(|id| format!("local:{id}"))
    }
    pub fn snapshot(&self, id: &str) -> Result<&MapSnapshot> {
        self.snapshots
            .get(id)
            .ok_or_else(|| anyhow!("Unknown local scenegraph"))
    }
    /// Discovery needs only the live frame identity, not a cloned voxel map or
    /// disk-backed annotation/volume views on every scheduling tick.
    pub fn frame_id(&self, id: &str) -> Result<String> {
        let snapshot = self.snapshot(id)?;
        let views = self.mobile_views.lock().map_err(|_| anyhow!("Local scene view unavailable"))?;
        Ok(views.get(id).and_then(|v| v["frame"].as_str())
            .unwrap_or(&snapshot.scenegraph.map.frame.id).to_owned())
    }
    pub fn view(&self, id: &str) -> Result<Value> {
        let snapshot = self.snapshot(id)?;
        if let Some(view) = self
            .mobile_views
            .lock()
            .map_err(|_| anyhow!("Local scene view unavailable"))?
            .get(id)
        {
            let mut view = view.clone();
            view["paths"] = serde_json::to_value(self.paths.get(id, &snapshot.scenegraph.map.frame.id)?)?;
            view["semantic_volumes"] = serde_json::to_value(self.volumes.get(id, &snapshot.scenegraph.map.frame.id)?)?;
            return Ok(view);
        }
        let map = &snapshot.scenegraph.map;
        let landmarks = crate::landmarks::portal_map(snapshot, None, &BTreeMap::new())?;
        Ok(
            json!({"environment_name":map.name,"frame":map.frame.id,"local_reference":true,
            "frame_description":map.frame.origin_description,"convention":"OpenGL · meters",
            "voxel_size":crate::collaborative_mapper::HOME_VOXEL_SIZE,"voxels":[],"portals":snapshot.scenegraph.anchors.values().collect::<Vec<_>>(),
            "paths":self.paths.get(id,&map.frame.id)?,"landmarks":landmarks,"semantic_volumes":self.volumes.get(id, &map.frame.id)?,"objects":[],"notes":[],"sources":{},"integrated_observations":0,"demo":false}),
        )
    }

    pub fn connection(
        &self,
        id: &str,
        mut home: crate::network::HomeConnection,
    ) -> Result<crate::network::HomeConnection> {
        let snapshot = self.snapshot(id)?;
        ensure!(
            frame_scene_id(&snapshot.scenegraph.map.frame.id) == Some(id),
            "Scene does not support phone calibration"
        );
        home.frame = snapshot.scenegraph.map.frame.id.clone();
        home.product_id = mobile_product(id);
        Ok(home)
    }

    /// Export the standard SDK Map Product and read operations on Home's peer.
    /// No remote write operation is exported; no shadow copies in the browser.
    pub fn mount(
        &self,
        runtime: &ComponentRuntime,
        endpoint: &auki_component_protocol::ComponentProtocolEndpoint,
        session_clock: auki_time::SessionClock,
    ) -> Result<Vec<MapComponent>> {
        let clock = crate::model::clock_ref(&session_clock.registry_entry());
        let mut components = Vec::new();
        for (id, snapshot) in &self.snapshots {
            let owner = runtime.peer_id().to_owned();
            let allowed = owner.clone();
            let component = MapComponent::new(
                runtime,
                MapComponentConfig {
                    component_id: format!("local-scene-{id}"),
                    publication_id: uuid::Uuid::new_v4().to_string(),
                    clock: clock.clone(),
                    map: snapshot.scenegraph.map.clone(),
                },
                { let clock = session_clock.clone(); move || clock.now_ns() },
                |_| true,
                move |ctx| ctx.caller_peer_id == allowed,
            )?;
            for anchor in snapshot.scenegraph.anchors.values() {
                InMemoryTransport
                    .invoke(
                        component.upsert_qr(),
                        InvocationContext {
                            invocation_id: uuid::Uuid::new_v4().to_string(),
                            caller_peer_id: owner.clone(),
                            caller_component_id: format!("local-scene-{id}"),
                        },
                        UpsertQr {
                            expected_snapshot: component.snapshot_reference(),
                            anchor: anchor.clone(),
                        },
                    )
                    .map_err(|e| anyhow!("{e:?}"))?;
            }
            endpoint.export_product(&component.product())?;
            endpoint.export_operable(component.lookup_qr())?;
            endpoint.export_operable(component.find_qr())?;
            components.push(component);
        }
        Ok(components)
    }
}
