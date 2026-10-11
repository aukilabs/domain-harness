//! Shared phone annotations: Home validates the ray against its canonical map.
use crate::{
    model::{FRAME, Store},
    notes::{SurfaceAnchor, TextNote},
    raycast::{Hit, Voxel},
};
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};

pub const COMPONENT: &str = "home-annotations";
pub const OPERATION: &str = "place";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeNote {
    pub request_id: String,
    pub frame: String,
    pub action: NoteAction,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NoteAction {
    PutVolume {
        volume: crate::semantic_volumes::SemanticVolume,
    },
    DeleteVolume {
        id: String,
    },
    Place {
        id: String,
        origin: [f64; 3],
        position: [f64; 3],
        text: String,
    },
    Edit {
        id: String,
        text: String,
    },
    Delete {
        id: String,
    },
    Restore {
        note: TextNote,
    },
}
#[derive(Clone, Serialize, Deserialize)]
pub struct NoteChange {
    pub request_id: String,
    pub note: Option<TextNote>,
    pub deleted: bool,
}
/// SDK invocation IDs identify requests but do not themselves deduplicate writes.
/// Remember successful results for five minutes, bounded by both count and bytes.
#[derive(Default)]
pub struct ChangeMemo {
    entries: std::collections::VecDeque<(
        String,
        String,
        Vec<u8>,
        NoteChange,
        std::time::Instant,
        usize,
    )>,
}
impl ChangeMemo {
    pub fn run(
        &mut self,
        caller: &str,
        request: &ChangeNote,
        apply: impl FnOnce() -> Result<NoteChange>,
    ) -> Result<NoteChange> {
        let encoded = serde_json::to_vec(request)?;
        ensure!(
            encoded.len() <= 2 * 1024 * 1024,
            "Annotation request too large"
        );
        self.entries
            .retain(|e| e.4.elapsed() < std::time::Duration::from_secs(300));
        if let Some(entry) = self
            .entries
            .iter()
            .find(|e| e.0 == caller && e.1 == request.request_id)
        {
            ensure!(entry.2 == encoded, "Annotation request ID already used");
            return Ok(entry.3.clone());
        }
        let result = apply()?;
        let bytes = encoded.len() + serde_json::to_vec(&result)?.len();
        while !self.entries.is_empty()
            && (self.entries.len() >= 128
                || self.entries.iter().map(|e| e.5).sum::<usize>() + bytes > 4 * 1024 * 1024)
        {
            self.entries.pop_front();
        }
        self.entries.push_back((
            caller.into(),
            request.request_id.clone(),
            encoded,
            result.clone(),
            std::time::Instant::now(),
            bytes,
        ));
        Ok(result)
    }
}
impl auki_components::ContractType for ChangeNote {
    const DATATYPE: &'static str = "spatial.home-change-note/v1";
}
impl auki_components::ContractType for NoteChange {
    const DATATYPE: &'static str = "spatial.note-change/v1";
}
pub fn change_contract() -> auki_components::OperableContract {
    use auki_components::ContractType;
    auki_components::OperableContract {
        name: "change".into(),
        instruction: ChangeNote::DATATYPE.into(),
        result: NoteChange::DATATYPE.into(),
        exposure: auki_components::Exposure::Cluster,
    }
}

/// Work on a copy: validation/persistence failure cannot partially change live notes.
pub fn apply_change(
    notes: &crate::notes::Notes,
    request: &ChangeNote,
    frame: &str,
) -> Result<(crate::notes::Notes, NoteChange)> {
    ensure!(
        request.frame == frame,
        "Annotation belongs to another scene"
    );
    uuid::Uuid::parse_str(&request.request_id)?;
    let mut next = notes.clone();
    let editable = |note: &TextNote| -> Result<()> {
        ensure!(
            note.anchor_robot.is_none(),
            "Device poses cannot be edited or deleted"
        );
        Ok(())
    };
    let (note, deleted) = match &request.action {
        NoteAction::PutVolume { volume } => {
            volume.validate()?;
            if let Some(old) = next.volumes.iter_mut().find(|v| v.id == volume.id) {
                *old = volume.clone();
            } else {
                next.volumes.push(volume.clone());
            }
            (None, false)
        }
        NoteAction::DeleteVolume { id } => {
            uuid::Uuid::parse_str(id)?;
            next.volumes.retain(|v| v.id != *id);
            (None, true)
        }
        NoteAction::Place {
            id,
            origin,
            position,
            text,
        } => {
            uuid::Uuid::parse_str(id)?;
            ensure!(
                origin
                    .iter()
                    .chain(position)
                    .all(|v| v.is_finite() && v.abs() < 1_000_000.),
                "Invalid surface hit"
            );
            let distance = origin
                .iter()
                .zip(position)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                .sqrt();
            ensure!(
                (0.05..=40.).contains(&distance),
                "Surface must be within 40 metres"
            );
            let note = TextNote {
                id: id.clone(),
                text: text.trim().into(),
                details: Default::default(),
                position: *position,
                anchor_portal: String::new(),
                anchor_robot: None,
                height_m: 0.,
                anchor_surface: Some(SurfaceAnchor {
                    frame: frame.into(),
                    voxel_center: *position,
                    beam_diameter_m: 0.,
                    floor: false,
                }),
            };
            if let Some(existing) = next.items.iter().find(|n| n.id == *id) {
                ensure!(existing == &note, "Annotation ID already used");
            } else {
                next.items.push(note.clone());
            }
            (Some(note), false)
        }
        NoteAction::Edit { id, text } => {
            let note = next
                .items
                .iter_mut()
                .find(|n| n.id == *id)
                .ok_or_else(|| anyhow!("Annotation no longer exists"))?;
            editable(note)?;
            note.text = text.trim().into();
            // Home's UI prefers title when present; keep both presentations coherent.
            if !note.details.title.is_empty() {
                note.details.title = note.text.clone();
            }
            (Some(note.clone()), false)
        }
        NoteAction::Delete { id } => {
            let old = if let Some(index) = next.items.iter().position(|n| n.id == *id) {
                editable(&next.items[index])?;
                Some(next.items.remove(index))
            } else {
                None
            };
            (old, true)
        }
        NoteAction::Restore { note } => {
            editable(note)?;
            ensure!(
                note.anchor_surface
                    .as_ref()
                    .is_none_or(|s| s.frame == frame),
                "Annotation belongs to another scene"
            );
            if let Some(existing) = next.items.iter().find(|n| n.id == note.id) {
                ensure!(existing == note, "Annotation ID already used");
            } else {
                next.items.push(note.clone());
            }
            (Some(note.clone()), false)
        }
    };
    next.validate(frame)?;
    Ok((
        next,
        NoteChange {
            request_id: request.request_id.clone(),
            note,
            deleted,
        },
    ))
}

pub async fn send_change(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
    request: &ChangeNote,
) -> Result<NoteChange> {
    let operations = crate::mobile_home::HomeOperations::new(peer,home)?;
    send_change_via(&operations,home,request).await
}

pub async fn send_change_via(
    operations: &crate::mobile_home::HomeOperations,
    home: &crate::network::HomeConnection,
    request: &ChangeNote,
) -> Result<NoteChange> {
    ensure!(
        request.frame == home.frame && request.frame == operations.frame,
        "Annotation belongs to another scene"
    );
    operations.invoke(&home.scoped_component(COMPONENT),"change","phone-annotations",
        &request.request_id,std::time::Duration::from_secs(10),request).await
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceNote {
    pub id: String,
    pub frame: String,
    pub origin: [f64; 3],
    pub direction: [f64; 3],
    pub text: String,
}
impl auki_components::ContractType for PlaceNote {
    const DATATYPE: &'static str = "spatial.home-place-note/v1";
}
impl auki_components::ContractType for TextNote {
    const DATATYPE: &'static str = "spatial.text-note/v1";
}

pub fn intersection(
    voxels: &[Voxel],
    size: f64,
    origin: [f64; 3],
    direction: [f64; 3],
) -> Result<Option<(Hit, bool)>> {
    let voxel = crate::raycast::raycast(voxels, size, origin, direction, 40.)?;
    let length = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
    let d = direction.map(|v| v / length);
    let floor = if d[1].abs() > 1e-8 {
        let distance = -origin[1] / d[1];
        if distance > 0.001 && distance <= 40. {
            let mut position = std::array::from_fn(|i| origin[i] + d[i] * distance);
            position[1] = 0.;
            Some(Hit {
                position,
                center: position,
                distance,
            })
        } else {
            None
        }
    } else {
        None
    };
    Ok(match (voxel, floor) {
        (Some(v), Some(f)) if f.distance < v.distance => Some((f, true)),
        (Some(v), _) => Some((v, false)),
        (_, Some(f)) => Some((f, true)),
        _ => None,
    })
}
pub fn place(store: &mut Store, request: PlaceNote) -> Result<TextNote> {
    place_with_live(store, request, None)
}
pub fn place_with_live(
    store: &mut Store,
    request: PlaceNote,
    live: Option<&crate::network::CanonicalScene>,
) -> Result<TextNote> {
    ensure!(
        request.frame == FRAME,
        "Annotation must use Home's calibrated frame"
    );
    uuid::Uuid::parse_str(&request.id)?;
    if let Some(note) = crate::notes::from_registries(&store.built.registries, FRAME)?
        .items
        .into_iter()
        .find(|n| n.id == request.id)
    {
        ensure!(
            note.text == request.text.trim(),
            "Annotation ID already used"
        );
        return Ok(note);
    }
    let (voxels, size) = if let Some(scene) = live {
        let view = crate::mobile_geometry::render(scene)?;
        (
            serde_json::from_value::<Vec<Voxel>>(view["voxels"].clone())?,
            scene.voxels.definition.grid().voxel_size_m.0,
        )
    } else {
        (
            crate::model::occupied(&store.built.voxels)?,
            store.built.view.voxel_size,
        )
    };
    let (hit, floor) = intersection(&voxels, size, request.origin, request.direction)?
        .ok_or_else(|| anyhow!("No floor or voxel within 40 metres"))?;
    crate::notes::save(
        store,
        TextNote {
            id: request.id,
            text: request.text.trim().into(),
            details: crate::notes::NoteDetails::default(),
            position: hit.position,
            anchor_portal: String::new(),
            anchor_robot: None,
            height_m: 0.,
            anchor_surface: Some(SurfaceAnchor {
                frame: FRAME.into(),

                voxel_center: hit.center,
                beam_diameter_m: 0.,
                floor,
            }),
        },
    )
}

pub async fn send(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
    request: &PlaceNote,
) -> Result<TextNote> {
    use auki_component_protocol::{CatalogResponse, ComponentProtocolClient};
    let expected = home.validate(peer)?;
    let client = ComponentProtocolClient::new(peer.protocols());
    let mut target = None;
    for route in &home.routes {
        let route: auki_sdk::Multiaddr = route.parse()?;
        if let Ok(Ok(CatalogResponse::Snapshot { snapshot })) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.catalog_exact(expected, route.clone(), None),
        )
        .await
        {
            let entry = snapshot
                .components
                .iter()
                .find(|c| c.manifest.component_id == home.scoped_component(COMPONENT))
                .ok_or_else(|| {
                    anyhow!("Home needs the annotation update; reconnect after Home restarts")
                })?;
            target = Some((route, entry.manifest.reference()));
            break;
        }
    }
    let (route, component) = target.ok_or_else(|| anyhow!("Home is unavailable"))?;
    // Do not retry writes automatically. The stable note UUID makes a user's retry idempotent.
    let invocation = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.invoke_exact::<_, TextNote>(
            expected,
            route,
            component,
            OPERATION,
            "phone-annotations",
            &request.id,
            Some(std::time::Duration::from_secs(10)),
            request,
        ),
    )
    .await??;
    Ok(invocation.result)
}
