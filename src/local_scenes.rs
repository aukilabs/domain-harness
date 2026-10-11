//! Persisted, independent local SDK scenegraphs. Selecting one never changes
//! the legacy canonical mapper's calibration or journal.
use anyhow::{Result, anyhow, ensure};
use auki_components::{ClockReference, ComponentRuntime, InMemoryTransport, InvocationContext};
use auki_scenegraph::{
    CoordinateConvention, MapDefinition, MapFrame, MapSnapshot, QrAnchor, RigidTransform,
    Scenegraph,
    component::{MapComponent, MapComponentConfig, UpsertQr},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write, path::Path, time::Instant};

pub struct LocalScenes {
    pub snapshots: BTreeMap<String, MapSnapshot>,
    pub(crate) directory: std::path::PathBuf,
    pub(crate) mobile_views: std::sync::Mutex<BTreeMap<String, Value>>,
    pub(crate) phone_presence: std::sync::Mutex<BTreeMap<String, std::sync::Arc<std::sync::Mutex<crate::mobile_pose::Presence>>>>,
}

/// Stable physical-origin identity, not a coordinate-convention alias.
pub fn frame_scene_id(frame: &str) -> Option<&str> {
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
    pub fn load(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        // Seed only genuinely configured origin portals. Never clone Xmart.
        for env in &crate::environments::CATALOG.environments {
            let Some(origin) = &env.origin else {
                continue;
            };
            ensure!(valid_id(&env.id), "Invalid local scene ID");
            let path = directory.join(format!("{}.json", env.id));
            if path.exists() {
                continue;
            } // Existing saved scenes are authoritative.
            let sizes = crate::portal_sizes::PortalSizes::load(&env.portal_sizes)?;
            let size = sizes
                .encoded_size_m(&origin.portal_code)?
                .ok_or_else(|| anyhow!("Origin Portal size missing"))?;
            let frame = format!("local-scene:{}:portal-{}", env.id, origin.portal_code);
            let mut graph = Scenegraph::new(MapDefinition {
                map_id: env.id.clone(),
                name: Some(env.name.clone()),
                domain_reference: None,
                frame: MapFrame::in_convention(
                    &frame,
                    &origin.frame_description,
                    CoordinateConvention::OpenGl,
                    1.,
                )?,
            })?;
            graph.anchors.insert(
                origin.portal_code.clone(),
                QrAnchor {
                    anchor_id: origin.portal_code.clone(),
                    payload: origin.portal_code.clone(),
                    side_length_m: size,
                    pose_in_map: RigidTransform {
                        from_frame_id: format!("portal:{}:local", origin.portal_code),
                        to_frame_id: frame,
                        translation: origin.translation_m,
                        rotation_wxyz: origin.rotation_wxyz,
                    },
                },
            );
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
            let snapshot: MapSnapshot = serde_json::from_slice(&std::fs::read(entry.path())?)?;
            snapshot.validate()?;
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
            snapshots,
            directory: directory.into(),
            mobile_views: Default::default(),
            phone_presence: Default::default(),
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
    pub fn view(&self, id: &str) -> Result<Value> {
        let snapshot = self.snapshot(id)?;
        if let Some(view) = self
            .mobile_views
            .lock()
            .map_err(|_| anyhow!("Local scene view unavailable"))?
            .get(id)
        {
            return Ok(view.clone());
        }
        let map = &snapshot.scenegraph.map;
        let landmarks = crate::landmarks::portal_map(snapshot, None, &BTreeMap::new())?;
        Ok(
            json!({"environment_name":map.name,"frame":map.frame.id,"local_reference":true,
            "frame_description":map.frame.origin_description,"convention":"OpenGL · meters",
            "voxel_size":0.1,"voxels":[],"portals":snapshot.scenegraph.anchors.values().collect::<Vec<_>>(),
            "landmarks":landmarks,"objects":[],"notes":[],"sources":{},"integrated_observations":0,"demo":false}),
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
        clock: ClockReference,
        started: Instant,
    ) -> Result<Vec<MapComponent>> {
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
                move || started.elapsed().as_nanos() as u64,
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
