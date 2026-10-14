//! Exact SDK Registry definitions required by Catalog outputs. No inline fallback.
use anyhow::{Result, ensure};
use auki_components::{CatalogSnapshot, ComponentRuntime};
use auki_protocols::registry::{RegistryClient, RegistryEndpoint, v3::*};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
pub type Definitions = Arc<Mutex<Vec<RegistryEntryEnvelope>>>;
pub fn resolved_contract(
    registry: &Value,
    c: &crate::compact_pose::Contract,
) -> Result<(
    auki_odometry::PoseContract,
    auki_components::clock::ClockRegistryEntry,
)> {
    let entries = registry["definitions"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Waiting for peer Registry definitions"))?;
    let frame = |r: &crate::compact_pose::FrameRef| -> Result<auki_registry::FrameRegistryEntry> {
        entries
            .values()
            .filter_map(|v| {
                serde_json::from_value::<auki_registry::FrameRegistryEntry>(v.clone()).ok()
            })
            .find(|f| crate::compact_pose::FrameRef::from(f) == *r)
            .ok_or_else(|| anyhow::anyhow!("Unresolved frame {} / {}", r.peer_id, r.id))
    };
    let clock = entries
        .values()
        .filter_map(|v| {
            serde_json::from_value::<auki_components::clock::ClockRegistryEntry>(v.clone()).ok()
        })
        .find(|v| crate::model::clock_ref(v) == c.clock)
        .ok_or_else(|| anyhow::anyhow!("Unresolved capture clock"))?;
    let contract = auki_odometry::PoseContract {
        from_frame: frame(&c.from_frame)?,
        to_frame: frame(&c.to_frame)?,
        clock: c.clock.clone(),
        session_id: c.session_id.clone(),
    };
    contract.validate()?;
    Ok((contract, clock))
}
pub fn clock(entries: &Definitions, c: &auki_components::clock::ClockRegistryEntry) -> Result<()> {
    insert(
        entries,
        RegistryEntryEnvelope {
            kind: RegistryKind::Clock,
            id: c.clock_id.clone(),
            hash: c.hash(),
            canonical_json: String::from_utf8(c.canonical_bytes())?,
        },
    )
}
pub fn insert(entries: &Definitions, e: RegistryEntryEnvelope) -> Result<()> {
    let mut entries = entries
        .lock()
        .map_err(|_| anyhow::anyhow!("Registry unavailable"))?;
    if !entries
        .iter()
        .any(|old| old.kind == e.kind && old.id == e.id && old.hash == e.hash)
    {
        entries.push(e);
    }
    Ok(())
}
pub fn frame(entries: &Definitions, f: &auki_registry::FrameRegistryEntry) -> Result<()> {
    insert(
        entries,
        RegistryEntryEnvelope {
            kind: RegistryKind::Frame,
            id: f.frame_id.clone(),
            hash: f.hash(),
            canonical_json: String::from_utf8(f.canonical_bytes())?,
        },
    )
}
pub fn mount(
    peer: &auki_sdk::AukiPeer,
    runtime: ComponentRuntime,
    entries: Definitions,
) -> Result<RegistryEndpoint> {
    Ok(RegistryEndpoint::mount(
        peer.protocols(),
        move |_: &auki_sdk::AuthenticatedPeer, r: &RegistryRequest| {
            // Every registered Output frame is fetchable through its owner's endpoint.
            for c in runtime.catalog().snapshot().components {
                for o in c.current_outputs.values() {
                    if let Some(f) = &o.manifest.spatial_frame {
                        if f.definition.peer_id == runtime.peer_id() {
                            let _ = frame(&entries, &f.definition);
                        }
                    }
                }
            }
            let Ok(entries) = entries.lock() else {
                return RegistryResponse::Error {
                    reason: "Registry unavailable".into(),
                };
            };
            match r {
                RegistryRequest::List { kind } => RegistryResponse::List {
                    entries: entries
                        .iter()
                        .filter(|e| e.kind == *kind)
                        .map(|e| RegistryListEntry {
                            id: e.id.clone(),
                            hash: e.hash.clone(),
                        })
                        .collect(),
                },
                RegistryRequest::Get { kind, id, hash } => RegistryResponse::Get {
                    entry: entries
                        .iter()
                        .find(|e| e.kind == *kind && e.id == *id && e.hash == *hash)
                        .cloned(),
                },
            }
        },
    )?)
}
pub async fn resolve(
    peer: &auki_sdk::AukiPeer,
    id: auki_sdk::PeerId,
    row: &Value,
    previous: Option<&Value>,
    local: &Definitions,
) -> Value {
    let work = async {
        let catalog: CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
        let mut refs = BTreeMap::new();
        for c in &catalog.components {
            for o in c.current_outputs.values() {
                o.manifest.validate().map_err(anyhow::Error::msg)?;
                ensure!(
                    o.manifest.peer_id == id.to_string(),
                    "Output owner mismatch"
                );
                refs.insert(
                    format!(
                        "clock:{}:{}:{}",
                        o.manifest.clock.peer_id, o.manifest.clock.id, o.manifest.clock.hash
                    ),
                    ("clock", serde_json::to_value(&o.manifest.clock)?),
                );
                if let Some(f) = &o.manifest.spatial_frame {
                    let r = serde_json::to_value(&f.reference)?;
                    refs.insert(
                        format!("frame:{}:{}:{}", r["peer_id"], r["id"], r["hash"]),
                        ("frame", r),
                    );
                }
            }
        }
        for p in &catalog.products {
            if let Some(m) = &p.metadata {
                if m.schema == crate::declared_depth::METADATA {
                    let r=m.value["frame"].clone();
                    refs.insert(format!("frame:{}:{}:{}",r["peer_id"],r["id"],r["hash"]),("frame",r));
                }
                if m.schema == crate::compact_pose::METADATA {
                    for key in ["from_frame", "to_frame"] {
                        let r = m.value[key].clone();
                        refs.insert(
                            format!("frame:{}:{}:{}", r["peer_id"], r["id"], r["hash"]),
                            ("frame", r),
                        );
                    }
                }
            }
        }
        ensure!(refs.len() <= 256, "Too many Registry references");
        let client = RegistryClient::new(peer.protocols());
        let route = row["catalog_route"]
            .as_str()
            .and_then(|s| s.parse::<auki_sdk::Multiaddr>().ok());
        let mut definitions = serde_json::Map::new();
        let mut errors = serde_json::Map::new();
        let began = std::time::Instant::now();
        // A broken early reference must not starve later Products on every refresh.
        let mut refs = refs.into_iter().collect::<Vec<_>>();
        let count = refs.len();
        let offset = previous
            .and_then(|v| v["next_offset"].as_u64())
            .unwrap_or(0) as usize
            % count.max(1);
        refs.rotate_left(offset);
        let mut next_offset = None;
        for (index, (key, (kind, r))) in refs.into_iter().enumerate() {
            if let Some(v) = previous.and_then(|p| p["definitions"].get(&key)) {
                definitions.insert(key, v.clone());
                continue;
            }
            if began.elapsed() > Duration::from_secs(9) {
                next_offset.get_or_insert((offset + index) % count.max(1));
                errors.insert(
                    key,
                    json!("Registry fetch deferred to next catalog refresh"),
                );
                continue;
            }
            let result = async {
                let owner: auki_sdk::PeerId = r["peer_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Registry owner missing"))?
                    .parse()?;
                let name = r["id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Registry id missing"))?;
                let hash = r["hash"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Registry hash missing"))?;
                if owner == peer.peer_id() {
                    let entries = local
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Local registry unavailable"))?;
                    let kind = if kind == "clock" {
                        RegistryKind::Clock
                    } else {
                        RegistryKind::Frame
                    };
                    let entry = entries
                        .iter()
                        .find(|e| e.kind == kind && e.id == name && e.hash == hash)
                        .ok_or_else(|| anyhow::anyhow!("Local registry definition not found"))?;
                    return Ok(serde_json::from_str(&entry.canonical_json)?);
                }
                let route = route.clone().filter(|_| owner == id);
                Ok::<Value, anyhow::Error>(if kind == "clock" {
                    serde_json::to_value(if let Some(route) = route {
                        client.fetch_clock_exact(owner, route, name, hash).await?
                    } else {
                        client.fetch_clock(owner, name, hash).await?
                    })?
                } else {
                    serde_json::to_value(if let Some(route) = route {
                        client.fetch_frame_exact(owner, route, name, hash).await?
                    } else {
                        client.fetch_frame(owner, name, hash).await?
                    })?
                })
            };
            match tokio::time::timeout(Duration::from_secs(2), result).await {
                Ok(Ok(v)) => {
                    definitions.insert(key, v);
                }
                e => {
                    errors.insert(key, json!(format!("{e:?}")));
                }
            }
        }
        Ok::<Value, anyhow::Error>(
            json!({"status":if errors.is_empty(){"ready"}else{"incomplete"},"definitions":definitions,"errors":errors,"next_offset":next_offset.unwrap_or(0)}),
        )
    };
    match tokio::time::timeout(Duration::from_secs(12), work).await {
        Ok(Ok(v)) => v,
        e => json!({"status":"unavailable","detail":format!("{e:?}")}),
    }
}
