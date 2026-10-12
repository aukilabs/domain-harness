//! Home-authored text annotations. PR 421's QR-only graph has no text node;
//! carry a typed extension in the existing canonical-scene registry envelope.
use crate::model::{FRAME, Store};
use anyhow::{Result, ensure};
use auki_registry::FrameRegistryEntry;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextNote {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub details: NoteDetails,
    pub position: [f64; 3],
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub anchor_portal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_robot: Option<RobotAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_surface: Option<SurfaceAnchor>,
    pub height_m: f64,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NoteDetails {
    pub name: String,
    pub title: String,
    pub description: String,
    pub photo: Option<String>,
    pub thumbnail: Option<String>,
}
impl NoteDetails {
    pub(crate) fn prepare_thumbnail(&mut self) -> Result<()> {
        use base64::Engine;
        self.validate()?;
        self.thumbnail = match &self.photo {
            Some(photo) => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(photo.trim_start_matches("data:image/jpeg;base64,"))?;
                let mut reader = image::ImageReader::with_format(
                    std::io::Cursor::new(bytes),
                    image::ImageFormat::Jpeg,
                );
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(8192);
                limits.max_image_height = Some(8192);
                reader.limits(limits);
                let thumbnail = reader.decode()?.thumbnail(320, 240);
                let mut encoded = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 75)
                    .encode_image(&thumbnail)?;
                Some(format!(
                    "data:image/jpeg;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(encoded)
                ))
            }
            None => None,
        };
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.name.chars().count() <= 120
                && self.title.chars().count() <= 120
                && self.description.chars().count() <= 10000,
            "Note fields are too long"
        );
        if let Some(photo) = &self.photo {
            use base64::Engine;
            ensure!(photo.len() <= 1_400_000, "Photo must be under 1 MB");
            let encoded = photo
                .strip_prefix("data:image/jpeg;base64,")
                .ok_or_else(|| anyhow::anyhow!("Photo must be a JPEG image"))?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
            ensure!(
                bytes.starts_with(&[0xff, 0xd8, 0xff]) && bytes.ends_with(&[0xff, 0xd9]),
                "Invalid JPEG photo"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobotAnchor {
    pub peer_id: String,
    pub capture_timestamp_ns: u64,
    pub clock: auki_components::ClockReference,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceAnchor {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub floor: bool,
    pub frame: String,

    pub voxel_center: [f64; 3],
    pub beam_diameter_m: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewNote {
    pub text: String,
    pub portal: String,
    pub height_m: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notes {
    pub schema: String,
    pub frame: String,
    pub items: Vec<TextNote>,
    /// Separate typed layer, persisted atomically with the scene's annotations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<crate::semantic_volumes::SemanticVolume>,
}
impl Notes {
    pub fn empty(frame: &str) -> Self {
        Self {
            schema: "spatial.text-notes/v1".into(),
            frame: frame.into(),
            items: vec![],
            volumes: vec![],
        }
    }
    pub fn validate(&self, frame: &str) -> Result<()> {
        ensure!(self.volumes.len() <= 256, "Too many semantic volumes");
        let mut volume_ids = std::collections::BTreeSet::new();
        for volume in &self.volumes {
            volume.validate()?;
            ensure!(volume_ids.insert(&volume.id), "Duplicate semantic volume ID");
        }
        ensure!(
            self.schema == "spatial.text-notes/v1"
                && self.frame == frame
                && self.items.len() <= 256,
            "Invalid text-note schema, frame or count"
        );
        let mut ids = std::collections::BTreeSet::new();
        for n in &self.items {
            n.details.validate()?;
            ensure!(
                !n.id.is_empty() && n.id.len() <= 128 && ids.insert(&n.id),
                "Invalid or duplicate note ID"
            );
            ensure!(
                !n.text.trim().is_empty()
                    && n.text.chars().count() <= 120
                    && !n.text.chars().any(char::is_control),
                "Use 1–120 printable characters for a note"
            );
            ensure!(
                n.position.iter().all(|v| v.is_finite())
                    && n.height_m.is_finite()
                    && n.height_m.abs() <= 100.
                    && !(n.anchor_robot.is_some() && n.anchor_surface.is_some())
                    && match &n.anchor_robot {
                        Some(robot) =>
                            n.anchor_portal.is_empty()
                                && !robot.peer_id.is_empty()
                                && robot.clock.peer_id == robot.peer_id,
                        None => match &n.anchor_surface {
                            Some(surface) =>
                                n.anchor_portal.is_empty()
                                    && (surface.frame.starts_with("home-live:")
                                        || surface.frame == FRAME
                                        || (surface.frame == frame
                                            && crate::local_scenes::frame_scene_id(frame)
                                                .is_some()))
                                    && surface.voxel_center.iter().all(|v| v.is_finite())
                                    && (surface.beam_diameter_m == 0.5
                                        || ((surface.frame == FRAME
                                            || (surface.frame == frame
                                                && crate::local_scenes::frame_scene_id(frame)
                                                    .is_some()))
                                            && surface.beam_diameter_m == 0.))
                                    && (!surface.floor
                                        || ((surface.frame == FRAME
                                            || (surface.frame == frame
                                                && crate::local_scenes::frame_scene_id(frame)
                                                    .is_some()))
                                            && surface.voxel_center[1] == 0.)),
                            None => n.anchor_portal.len() == 11,
                        },
                    },
                "Invalid note position or portal"
            );
        }
        Ok(())
    }
    pub fn in_frame(&self, from: &FrameRegistryEntry, to: &FrameRegistryEntry) -> Result<Self> {
        self.validate(&from.frame_id)?;
        let mut result = self.clone();
        result.frame = to.frame_id.clone();
        for n in &mut result.items {
            // Callers establish a shared physical origin before changing conventions.
            let p = auki_geometry::convert_point_convention(
                auki_datatypes::pose::Vec3 {
                    x: n.position[0],
                    y: n.position[1],
                    z: n.position[2],
                },
                from,
                to,
            )?;
            n.position = [p.x, p.y, p.z];
        }
        Ok(result)
    }
}
pub fn from_registries(registries: &Value, frame: &str) -> Result<Notes> {
    let notes: Notes = match registries.get("text_notes").filter(|v| !v.is_null()) {
        Some(v) => serde_json::from_value(v.clone())?,
        None => Notes::empty(frame),
    };
    notes.validate(frame)?;
    Ok(notes)
}
pub fn initialize(store: &mut Store) -> Result<()> {
    let path = store.path.with_extension("notes.json");
    let mut notes = if path.exists() {
        ensure!(
            // Authored notes may each contain a bounded, base64-encoded photo.
            std::fs::metadata(&path)?.len() <= 192 * 1024 * 1024,
            "Notes file too large"
        );
        serde_json::from_slice::<Notes>(&std::fs::read(&path)?)?
    } else {
        Notes::empty(FRAME)
    };
    notes.validate(FRAME)?;
    let before = notes.clone();
    for note in &mut notes.items {
        note.details.prepare_thumbnail()?;
    }
    if notes != before {
        crate::model::persist(&path, &notes)?;
    }
    store.built.registries["text_notes"] = json!(notes);
    Ok(())
}
pub fn add(store: &mut Store, request: NewNote) -> Result<TextNote> {
    add_detailed(store, request, NoteDetails::default())
}
pub fn add_detailed(store: &mut Store, request: NewNote, details: NoteDetails) -> Result<TextNote> {
    let portal = store
        .built
        .portals
        .scenegraph
        .anchors
        .get(&request.portal)
        .ok_or_else(|| anyhow::anyhow!("Unknown canonical portal"))?;
    let mut position = portal.pose_in_map.translation;
    position[1] += request.height_m;
    let note = TextNote {
        id: uuid::Uuid::new_v4().to_string(),
        text: request.text.trim().into(),
        details,
        position,
        anchor_portal: request.portal,
        anchor_robot: None,
        anchor_surface: None,
        height_m: request.height_m,
    };
    save(store, note)
}
pub fn add_surface(
    store: &mut Store,
    text: String,
    contact: crate::picking::Contact,
) -> Result<TextNote> {
    add_surface_detailed(store, text, contact, NoteDetails::default())
}
pub fn add_surface_detailed(
    store: &mut Store,
    text: String,
    contact: crate::picking::Contact,
    details: NoteDetails,
) -> Result<TextNote> {
    save(
        store,
        TextNote {
            id: uuid::Uuid::new_v4().to_string(),
            text: text.trim().into(),
            details,
            position: contact.position,
            anchor_portal: String::new(),
            anchor_robot: None,
            anchor_surface: Some(contact.anchor),
            height_m: 0.,
        },
    )
}
pub(crate) fn save(store: &mut Store, mut note: TextNote) -> Result<TextNote> {
    note.details.prepare_thumbnail()?;
    let mut notes = from_registries(&store.built.registries, FRAME)?;
    ensure!(
        notes.items.len() < 128,
        "Home supports up to 128 authored notes"
    );
    notes.items.push(note.clone());
    notes.validate(FRAME)?;
    crate::model::persist(&store.path.with_extension("notes.json"), &notes)?;
    store.built.registries["text_notes"] = json!(notes);

    Ok(note)
}
/// Delete an authored annotation; generated robot/presence labels are not stored here.
pub fn rename(store: &mut Store, id: &str, text: String) -> Result<TextNote> {
    update(store, id, text, None)
}
pub fn update(
    store: &mut Store,
    id: &str,
    text: String,
    details: Option<NoteDetails>,
) -> Result<TextNote> {
    let mut notes = from_registries(&store.built.registries, FRAME)?;
    let note = notes
        .items
        .iter_mut()
        .find(|n| n.id == id)
        .ok_or_else(|| anyhow::anyhow!("Annotation not found"))?;
    ensure!(
        note.anchor_robot.is_none(),
        "Live robot labels cannot be edited"
    );
    note.text = text.trim().into();
    if let Some(details) = details {
        note.details = details;
    }
    note.details.prepare_thumbnail()?;
    let result = note.clone();
    notes.validate(FRAME)?;
    crate::model::persist(&store.path.with_extension("notes.json"), &notes)?;
    store.built.registries["text_notes"] = json!(notes);

    Ok(result)
}
pub fn remove(store: &mut Store, id: &str) -> Result<()> {
    let mut notes = from_registries(&store.built.registries, FRAME)?;
    let index = notes
        .items
        .iter()
        .position(|n| n.id == id)
        .ok_or_else(|| anyhow::anyhow!("Annotation not found"))?;
    ensure!(
        notes.items[index].anchor_robot.is_none(),
        "Live robot labels cannot be deleted"
    );
    notes.items.remove(index);
    crate::model::persist(&store.path.with_extension("notes.json"), &notes)?;
    store.built.registries["text_notes"] = json!(notes);

    Ok(())
}
pub fn tool_edit(store:&mut Store,edit:crate::scene_tools::NoteEdit)->Result<serde_json::Value>{
    let mut notes=from_registries(&store.built.registries,FRAME)?;
    let receipt=crate::scene_tools::apply_note(&mut notes,edit,FRAME)?;
    crate::model::persist(&store.path.with_extension("notes.json"),&notes)?;
    store.built.registries["text_notes"]=json!(notes);Ok(receipt)
}
/// Home live is explicitly calibrated to the canonical origin, with Z-up axes.
pub fn attach_home(scene: &mut crate::network::CanonicalScene, notes: &Notes) -> Result<()> {
    ensure!(
        scene.frame.starts_with("home-live:")
            && scene.registries["local_mapping"]["origin"]["kind"] == "canonical_portals",
        "Notes require the shared Home origin"
    );
    let from = crate::conventions::registry(&scene.authority_peer_id, FRAME)?;
    scene.registries["text_notes"] = json!(notes.in_frame(&from, &scene.voxels.definition.frame)?);
    Ok(())
}

/// Stable robot identities, refreshed from pose evidence, not from repeated reads.
#[derive(Default)]
pub struct RobotLabels {
    seen: std::collections::BTreeMap<String, (RobotAnchor, std::time::Instant)>,
}
impl RobotLabels {
    pub fn update(
        &mut self,
        runners: &Value,
        names: &std::collections::BTreeMap<String, String>,
        frame: &FrameRegistryEntry,
        now: std::time::Instant,
    ) -> Result<Vec<TextNote>> {
        let from = crate::conventions::registry(&frame.peer_id, FRAME)?;
        let up = auki_geometry::convert_point_convention(
            auki_datatypes::pose::Vec3 {
                x: 0.,
                y: 1.8,
                z: 0.,
            },
            &from,
            frame,
        )?;
        let mut result = Vec::new();
        for (peer, name) in names {
            let runner = &runners[peer];
            if runner["status"] != "aligned" {
                self.seen.remove(peer);
                continue;
            }
            let robot = &runner["robot_pose"];
            if robot["frame"] != frame.frame_id {
                continue;
            }
            let (Ok(pose), Ok(clock), Some(timestamp)) = (
                serde_json::from_value::<auki_odometry::Pose>(robot["pose"].clone()),
                serde_json::from_value::<auki_components::ClockReference>(robot["clock"].clone()),
                robot["capture_timestamp_ns"].as_u64(),
            ) else {
                continue;
            };
            if pose.validate().is_err() || clock.peer_id != *peer {
                continue;
            }
            let anchor = RobotAnchor {
                peer_id: peer.clone(),
                capture_timestamp_ns: timestamp,
                clock,
            };
            let seen = self
                .seen
                .entry(peer.clone())
                .or_insert_with(|| (anchor.clone(), now));
            if seen.0.clock != anchor.clock
                || anchor.capture_timestamp_ns > seen.0.capture_timestamp_ns
            {
                *seen = (anchor.clone(), now);
            }
            if seen.0 != anchor || now.duration_since(seen.1) >= std::time::Duration::from_secs(3) {
                continue;
            }
            result.push(TextNote {
                id: format!("robot-name:{peer}"),
                text: name.clone(),
                details: NoteDetails::default(),
                position: [
                    pose.translation[0] + up.x,
                    pose.translation[1] + up.y,
                    pose.translation[2] + up.z,
                ],
                anchor_portal: String::new(),
                anchor_robot: Some(anchor),
                anchor_surface: None,
                height_m: 1.8,
            });
        }
        Ok(result)
    }
}
