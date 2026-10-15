//! Read actual peer catalogs without blocking Home's mapping/publication loop.
use auki_component_protocol::{CatalogResponse, ComponentProtocolClient, CATALOG_PROTOCOL_ID};
use auki_sdk::{AukiPeer, AukiPeerIdentity, Multiaddr, PeerId};
use futures::{stream::FuturesUnordered, StreamExt, FutureExt};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub type Catalogs = BTreeMap<String, Value>;

// Catalog availability is not stream liveness. Preserve authenticated product
// references across brief discovery/refresh failures; never refresh this lease
// on failure. Frame freshness and stream authentication remain independently
// enforced by consumers.
const CATALOG_GRACE: Duration = Duration::from_secs(60);

fn refreshed(previous: Option<&Value>, result: Value, age: Option<Duration>) -> Value {
    if result["catalog_status"] == "ready" {
        return result; // Including empty catalogs and changed manifests.
    }
    if let Some(previous) =
        previous.filter(|p| p["catalog"].is_object() && age.is_some_and(|age| age < CATALOG_GRACE))
    {
        let mut cached = previous.clone();
        cached["catalog_cached"] = json!(true);
        cached["catalog_refresh_status"] = json!("unavailable");
        cached["catalog_detail"] =
            json!("Catalog refresh unavailable; using last known catalog temporarily");
        return cached;
    }
    result
}

pub async fn fetch(peer: &AukiPeer, id: PeerId, configured: &[String]) -> Value {
    fetch_revision(peer, id, configured, None).await
}

pub async fn fetch_revision(peer: &AukiPeer, id: PeerId, configured: &[String], previous: Option<&Value>) -> Value {
    let revision = previous.and_then(|v| v["catalog"]["revision"].as_u64());
    let snapshot = |reply| match reply {
        CatalogResponse::Snapshot { snapshot } => Some(serde_json::to_value(snapshot).unwrap()),
        CatalogResponse::Unchanged { revision: received } if Some(received) == revision => previous.map(|p| p["catalog"].clone()),
        _ => None,
    };
    let work = async {
        let client = ComponentProtocolClient::new(peer.protocols());
        for route in configured {
            let Ok(route) = route.parse::<Multiaddr>() else {
                continue;
            };
            if let Ok(Ok(reply)) = tokio::time::timeout(
                Duration::from_secs(2),
                client.catalog_exact(id, route.clone(), revision),
            ).await
                && let Some(snapshot) = snapshot(reply)
            {
                return Some((snapshot, Some(route.clone())));
            }
        }
        // The SDK may already have configured routes for this identity.
        if let Ok(Ok(reply)) =
            tokio::time::timeout(Duration::from_secs(2), client.catalog(id, revision)).await
            && let Some(snapshot) = snapshot(reply)
        {
            return Some((snapshot, None));
        }
        let candidates = peer
            .resolve(AukiPeerIdentity::Peer(id), CATALOG_PROTOCOL_ID)
            .await
            .ok()?;
        for candidate in candidates {
            for route in candidate.candidate().routes() {
                // A discovered address is only a hint; exact dialing pins the
                // authenticated identity, never trusting the address as identity.
                if let Ok(Ok(reply)) = tokio::time::timeout(
                    Duration::from_secs(2),
                    client.catalog_exact(id, route.clone(), revision),
                ).await
                    && let Some(snapshot) = snapshot(reply)
                {
                    return Some((snapshot, Some(route.clone())));
                }
            }
        }
        None
    };
    match tokio::time::timeout(Duration::from_secs(8), work).await {
        Ok(Some((snapshot, route))) => {
            json!({"catalog_status":"ready","catalog":snapshot,"catalog_route":route.map(|r: Multiaddr|r.to_string())})
        }
        _ => {
            json!({"catalog_status":"unavailable","catalog_detail":"Catalog unavailable; peer may not advertise a reachable component endpoint"})
        }
    }
}

pub fn device_rows(peer: &AukiPeer, catalogs: &Catalogs) -> Vec<Value> {
    let mut rows: BTreeMap<_, _> = peer
        .known_peers()
        .snapshot()
        .peers()
        .iter()
        .map(|p| {
            let id = p.peer_id().to_string();
            let mut row = catalogs
                .get(&id)
                .cloned()
                .unwrap_or_else(|| json!({"catalog_status":"loading"}));
            row["peer_id"] = json!(id);
            row["status"] = json!("connected");
            (id, row)
        })
        .collect();
    for (id, cached) in catalogs {
        if cached["discovered"] == true && !rows.contains_key(id) {
            let mut row = cached.clone();
            row["peer_id"] = json!(id);
            row["status"] = json!("discovered");
            rows.insert(id.clone(), row);
        }
    }
    rows.into_values().collect()
}

// Observation counts/revisions change continuously; only definition changes
// require a new Registry pass. Full references prevent session mixing.
fn registry_signature(row: &Value) -> String {
    let products = row["catalog"]["products"].as_array().into_iter().flatten()
        .map(|p| json!([p["manifest_hash"], p["metadata"]["schema"], p["metadata"]["value"]])).collect::<Vec<_>>();
    let outputs = row["catalog"]["components"].as_array().into_iter().flatten()
        .map(|c| c["current_outputs"].clone()).collect::<Vec<_>>();
    json!([products, outputs]).to_string()
}

pub async fn monitor(
    peer: &AukiPeer,
    local_registry: &crate::product_registry::Definitions,
    scenes: &crate::local_scenes::LocalScenes,
    profiles: &[(String, Vec<String>)],
    output: watch::Sender<Catalogs>,
    stop: &CancellationToken,
) {
    // Poll the SDK's local authenticated-peer snapshot promptly; DDS discovery
    // runs independently and cannot delay a peer that is already connected.
    let mut last_success = BTreeMap::<String, Instant>::new();
    let mut due = BTreeMap::<PeerId, Instant>::new();
    let mut full_refresh = BTreeMap::<PeerId, Instant>::new();
    let mut connected = std::collections::BTreeSet::new();
    let mut discovered = BTreeMap::<PeerId, Vec<String>>::new();
    let mut busy = std::collections::BTreeSet::new();
    let mut fetches: FuturesUnordered<futures::future::BoxFuture<'_, (PeerId, Value)>> = FuturesUnordered::new();
    let mut registry_fetches: FuturesUnordered<futures::future::BoxFuture<'_, (PeerId, String, Option<Value>)>> = FuturesUnordered::new();
    let mut registry_busy = BTreeMap::<PeerId, (String, CancellationToken)>::new();
    let mut registry_done = BTreeMap::<PeerId, (String, Instant)>::new();
    let mut registered_frames = std::collections::BTreeSet::new();
    let mut discoveries = FuturesUnordered::new();
    // Seed before select so the SDK's concrete discovery result is inferred.
    discoveries.push(tokio::time::timeout(Duration::from_secs(3), peer.discover()));
    let mut next_discovery = Instant::now() + Duration::from_secs(5);
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            Some(result) = discoveries.next(), if !discoveries.is_empty() => {
                if let Ok(Ok(peers)) = result {
                    discovered = peers.into_iter().filter(|p| p.peer_id() != peer.peer_id())
                        .map(|p| (p.peer_id(), p.routes().iter().map(ToString::to_string).collect())).collect();
                }
                next_discovery = Instant::now() + Duration::from_secs(5);
            }
            Some((id, mut value)) = fetches.next(), if !fetches.is_empty() => {
                busy.remove(&id);
                due.insert(id, Instant::now() + if connected.contains(&id) { Duration::from_secs(1) } else { Duration::from_secs(5) });
                let key = id.to_string();
                if value["catalog_status"] == "ready" { last_success.insert(key.clone(), Instant::now()); }
                let age = last_success.get(&key).map(Instant::elapsed);
                output.send_modify(|rows| {
                    if value["catalog_status"] == "ready" { value["registries"] = rows.get(&key).map(|r| r["registries"].clone()).unwrap_or_else(|| json!({"status":"loading"})); }
                    let value = refreshed(rows.get(&key), value, age); rows.insert(key, value);
                });
            }
            Some((id, signature, result)) = registry_fetches.next(), if !registry_fetches.is_empty() => {
                registry_busy.remove(&id);
                if let Some(result) = result {
                    registry_done.insert(id, (signature.clone(), Instant::now()));
                    output.send_modify(|rows| { if let Some(row) = rows.get_mut(&id.to_string()) {
                        if registry_signature(row) == signature { row["registries"] = result; }
                    } });
                }
            }
            _ = tick.tick() => {}
        }
        if discoveries.is_empty() && Instant::now() >= next_discovery {
            discoveries.push(tokio::time::timeout(Duration::from_secs(3), peer.discover()));
        }
        for id in scenes.snapshots.keys() {
            if let Ok(frame) = scenes.frame_id(id) {
                if !registered_frames.contains(&frame) {
                    if let Ok(definition) = crate::conventions::registry(&peer.peer_id().to_string(), &frame) {
                        if crate::product_registry::frame(local_registry, &definition).is_ok() { registered_frames.insert(frame); }
                    }
                }
            }
        }
        let mut candidates: BTreeMap<PeerId, (Vec<String>, bool)> = BTreeMap::new();
        for (id, routes) in &discovered {
            candidates.insert(*id, (routes.clone(), true));
        }
        let mut now_connected = std::collections::BTreeSet::new();
        for p in peer.known_peers().snapshot().peers() {
            candidates.entry(p.peer_id()).or_default();
            now_connected.insert(p.peer_id());
            if !connected.contains(&p.peer_id()) {
                due.remove(&p.peer_id());
                full_refresh.remove(&p.peer_id());
            }
        }
        connected = now_connected;
        for (id, routes) in profiles {
            if let Ok(id) = id.parse() {
                candidates.entry(id).or_default().0.extend(routes.clone());
            }
        }
        let mut current = output.borrow().clone();
        // Discovery can fail independently of a healthy authenticated stream.
        // Retry recent peers using their last authenticated catalog route.
        for (id, row) in &current {
            if last_success
                .get(id)
                .is_some_and(|t| t.elapsed() < CATALOG_GRACE)
            {
                if let Ok(peer_id) = id.parse() {
                    let entry = candidates
                        .entry(peer_id)
                        .or_insert_with(|| (Vec::new(), row["discovered"] == true));
                    if let Some(route) = row["catalog_route"].as_str() {
                        if !entry.0.iter().any(|r| r == route) {
                            entry.0.push(route.into());
                        }
                    }
                }
            }
        }
        current.retain(|id, _| candidates.keys().any(|p| p.to_string() == *id) || id.parse().is_ok_and(|id| busy.contains(&id)));
        last_success.retain(|id, _| current.contains_key(id));
        for (id, (_, discovered)) in &candidates {
            current
                .entry(id.to_string())
                .or_insert_with(|| json!({"catalog_status":"loading","discovered":discovered}));
        }
        if *output.borrow() != current { output.send_replace(current); }
        let previous=output.borrow().clone();
        // Registry fetches have their own budget. A slow sensor definition
        // must not delay discovery of a newly published alignment Product.
        for (id, (signature, cancel)) in &registry_busy {
            if previous.get(&id.to_string()).is_none_or(|r| registry_signature(r) != *signature) { cancel.cancel(); }
        }
        registry_done.retain(|id, _| previous.contains_key(&id.to_string()));
        let mut registry_candidates = previous.iter().collect::<Vec<_>>();
        registry_candidates.sort_by_key(|(key, _)| key.parse::<PeerId>().ok().and_then(|id| registry_done.get(&id).map(|(_, at)| *at)));
        for (key, row) in registry_candidates {
            if registry_busy.len() >= 4 { break; }
            let Ok(id) = key.parse::<PeerId>() else { continue; };
            if row["catalog_status"] != "ready" || registry_busy.contains_key(&id) { continue; }
            let signature = registry_signature(row);
            if registry_done.get(&id).is_some_and(|(old, at)| old == &signature && (row["registries"]["status"] == "ready" || at.elapsed() < Duration::from_secs(1))) { continue; }
            let cancel = CancellationToken::new();
            registry_busy.insert(id, (signature.clone(), cancel.clone()));
            let row = row.clone(); let output = output.clone();
            registry_fetches.push(async move {
                let work = crate::product_registry::resolve_progress(peer, id, &row, Some(&row["registries"]), local_registry, |registry| {
                    output.send_modify(|rows| { if let Some(row) = rows.get_mut(&id.to_string()) {
                        if registry_signature(row) == signature { row["registries"] = registry; }
                    } });
                });
                let result = tokio::select! { _ = cancel.cancelled() => None, result = work => Some(result) };
                (id, signature, result)
            }.boxed());
        }
        due.retain(|id, _| candidates.contains_key(id) || busy.contains(id));
        full_refresh.retain(|id, _| candidates.contains_key(id) || busy.contains(id));
        let mut candidates = candidates.into_iter().filter(|(id, _)| !busy.contains(id) && due.get(id).is_none_or(|t| *t <= Instant::now())).collect::<Vec<_>>();
        candidates.sort_by_key(|(id, _)| due.get(id).copied());
        for (id, (mut routes, discovered)) in candidates.into_iter().take(4usize.saturating_sub(busy.len())) {
            let previous=previous.get(&id.to_string()).cloned();
            // Prefer the last authenticated route, and periodically force a full
            // snapshot in case a restarted endpoint reused a revision number.
            if let Some(route) = previous.as_ref().and_then(|p| p["catalog_route"].as_str()) {
                routes.retain(|r| r != route); routes.insert(0, route.to_owned());
            }
            let force = full_refresh.get(&id).is_none_or(|t| t.elapsed() >= Duration::from_secs(30));
            if force { full_refresh.insert(id, Instant::now()); }
            busy.insert(id);
            fetches.push(async move {
                let mut result = fetch_revision(peer, id, &routes, if force { None } else { previous.as_ref() }).await;
                result["discovered"] = json!(discovered);
                (id, result)
            }.boxed());
        }
    }
    // Scoped futures are cancelled before clearing state; no refresh can
    // resurrect a row after shutdown.
    drop(fetches);
    drop(registry_fetches);
    drop(discoveries);
    output.send_replace(Catalogs::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_identity_ignores_observation_updates_but_not_contract_changes() {
        let old=json!({"catalog":{"revision":1,"components":[],"products":[{"manifest_hash":"a","state":{"entries":1},"metadata":{"schema":"pose","source_sequence":1,"value":{"from_frame":"native"}}}]}});
        let mut new=old.clone();new["catalog"]["revision"]=json!(2);
        new["catalog"]["products"][0]["metadata"]["source_sequence"]=json!(2);
        new["catalog"]["products"][0]["state"]["entries"]=json!(2);
        assert_eq!(registry_signature(&old),registry_signature(&new));
        new["catalog"]["products"][0]["metadata"]["value"]["from_frame"]=json!("new-session");
        assert_ne!(registry_signature(&old),registry_signature(&new));
    }
    #[test]
    fn refresh_failure_retains_references_but_not_forever() {
        let ready = json!({"catalog_status":"ready", "catalog_route":"route", "catalog":{"products":[{"manifest_hash":"old"}]}});
        let failure = json!({"catalog_status":"unavailable"});
        let cached = refreshed(Some(&ready), failure.clone(), Some(Duration::from_secs(5)));
        assert_eq!(cached["catalog"], ready["catalog"]);
        assert_eq!(cached["catalog_route"], ready["catalog_route"]);
        assert_eq!(cached["catalog_status"], "ready");
        assert_eq!(cached["catalog_cached"], true);
        assert_eq!(
            refreshed(Some(&cached), failure.clone(), Some(CATALOG_GRACE)),
            failure
        );
        assert_eq!(refreshed(None, failure.clone(), None), failure);
    }
    #[test]
    fn successful_refresh_immediately_replaces_or_removes_old_products() {
        let old = json!({"catalog_status":"ready","catalog_cached":true,"catalog":{"products":[{"manifest_hash":"old"}]}});
        for products in [json!([]), json!([{"manifest_hash":"new"}])] {
            let new = json!({"catalog_status":"ready","catalog":{"products":products}});
            assert_eq!(
                refreshed(Some(&old), new.clone(), Some(Duration::ZERO)),
                new
            );
        }
    }
}
