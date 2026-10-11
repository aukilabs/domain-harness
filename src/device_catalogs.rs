//! Read actual peer catalogs without blocking Home's mapping/publication loop.
use auki_component_protocol::{CatalogResponse, ComponentProtocolClient, CATALOG_PROTOCOL_ID};
use auki_sdk::{AukiPeer, AukiPeerIdentity, Multiaddr, PeerId};
use futures::{stream, StreamExt};
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
    let work = async {
        let client = ComponentProtocolClient::new(peer.protocols());
        for route in configured {
            let Ok(route) = route.parse::<Multiaddr>() else {
                continue;
            };
            if let Ok(Ok(CatalogResponse::Snapshot { snapshot })) = tokio::time::timeout(
                Duration::from_secs(2),
                client.catalog_exact(id, route.clone(), None),
            )
            .await
            {
                return Some((snapshot, Some(route.clone())));
            }
        }
        // The SDK may already have configured routes for this identity.
        if let Ok(Ok(CatalogResponse::Snapshot { snapshot })) =
            tokio::time::timeout(Duration::from_secs(2), client.catalog(id, None)).await
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
                if let Ok(Ok(CatalogResponse::Snapshot { snapshot })) = tokio::time::timeout(
                    Duration::from_secs(2),
                    client.catalog_exact(id, route.clone(), None),
                )
                .await
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

pub async fn monitor(
    peer: &AukiPeer,
    profiles: &[(String, Vec<String>)],
    output: watch::Sender<Catalogs>,
    stop: &CancellationToken,
) {
    let mut last_success = BTreeMap::<String, Instant>::new();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! { _ = stop.cancelled() => break, _ = tick.tick() => {} }
        let mut candidates: BTreeMap<PeerId, (Vec<String>, bool)> = BTreeMap::new();
        let discovery = tokio::select! {
            _ = stop.cancelled() => break,
            result = tokio::time::timeout(Duration::from_secs(3), peer.discover()) => result,
        };
        if let Ok(Ok(discovered)) = discovery {
            for p in discovered {
                if p.peer_id() != peer.peer_id() {
                    candidates.insert(
                        p.peer_id(),
                        (p.routes().iter().map(ToString::to_string).collect(), true),
                    );
                }
            }
        }
        for p in peer.known_peers().snapshot().peers() {
            candidates.entry(p.peer_id()).or_default();
        }
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
        current.retain(|id, _| candidates.keys().any(|p| p.to_string() == *id));
        last_success.retain(|id, _| current.contains_key(id));
        for (id, (_, discovered)) in &candidates {
            current
                .entry(id.to_string())
                .or_insert_with(|| json!({"catalog_status":"loading","discovered":discovered}));
        }
        output.send_replace(current);
        let mut fetches = stream::iter(candidates.into_iter().map(
            |(id, (routes, discovered))| async move {
                let mut result = fetch(peer, id, &routes).await;
                result["discovered"] = json!(discovered);
                (id.to_string(), result)
            },
        ))
        .buffer_unordered(4);
        loop {
            let result = tokio::select! { _ = stop.cancelled() => return, result = fetches.next() => result };
            let Some((id, value)) = result else {
                break;
            };
            if value["catalog_status"] == "ready" {
                last_success.insert(id.clone(), Instant::now());
            }
            let age = last_success.get(&id).map(Instant::elapsed);
            output.send_modify(|catalogs| {
                let value = refreshed(catalogs.get(&id), value, age);
                catalogs.insert(id, value);
            });
        }
    }
    output.send_replace(Catalogs::new());
}

#[cfg(test)]
mod tests {
    use super::*;
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
