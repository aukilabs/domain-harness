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
            position: hit.position,
            anchor_portal: String::new(),
            anchor_robot: None,
            height_m: 0.,
            anchor_surface: Some(SurfaceAnchor {
                frame: FRAME.into(),
                revision: store.built.view.revision,
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
                .find(|c| c.manifest.component_id == COMPONENT)
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
