//! Held frame relationships through the compact pose profile. Trust policy:
//! docs/frame-registry-consumer.md#authority-and-discovery.
//! Authenticated Domain publishers may locate their own frame in Home; this is
//! a publisher claim, not independent evidence of localization accuracy.
use crate::compact_pose::{Contract, Lifetime, Update};
use anyhow::{Result, ensure};
use auki_components::*;

pub fn map_tracking(
    update: &Update,
    alignment: &auki_odometry::Pose,
    age: std::time::Duration,
) -> Result<auki_odometry::Tracking> {
    update.validate()?;
    Ok(match &update.tracking {
        auki_odometry::Tracking::Valid { pose } if age < std::time::Duration::from_secs(2) => {
            auki_odometry::Tracking::Valid {
                pose: crate::local_mapper::compose(alignment, pose)?,
            }
        }
        _ => auki_odometry::Tracking::Lost,
    })
}

/// Exact references, not Product/component names, establish this two-edge path.
/// Trust policy: an authenticated Domain peer can assert the location of its own
/// native frame, never replace Home's definition or assert another peer's source.
pub fn validate_chain(
    c: &Contract,
    pose: &Contract,
    publisher: &str,
    home_peer: &str,
    frame: &str,
) -> Result<()> {
    ensure!(
        pose.lifetime == Lifetime::Sampled && c.lifetime == Lifetime::Held,
        "Expected sampled pose and held scene transform"
    );
    ensure!(
        pose.from_frame.peer_id == publisher
            && pose.to_frame.peer_id == publisher
            && c.from_frame.peer_id == publisher
            && c.clock.peer_id == publisher,
        "Transform authority mismatch"
    );
    ensure!(
        c.session_id == pose.session_id && c.from_frame == pose.to_frame && c.clock == pose.clock,
        "Transform session/frame/clock mismatch"
    );
    ensure!(
        c.to_frame == (&crate::conventions::registry(home_peer, frame)?).into(),
        "Transform targets a different Home frame identity"
    );
    Ok(())
}

/// Select the latest state <= capture time, including lost. Never search back
/// past a lost state, use a future calibration, or expire a held state by age.
pub fn held_at(samples: &[(u64, Update)], timestamp: u64) -> Result<(auki_odometry::Pose, u64)> {
    ensure!(
        samples.windows(2).all(|w| w[0].0 < w[1].0),
        "Non-monotonic held transform history"
    );
    let (at, state) = samples
        .iter()
        .rev()
        .find(|(at, _)| *at <= timestamp)
        .ok_or_else(|| anyhow::anyhow!("No retained scene transform at capture time"))?;
    state.validate()?;
    match &state.tracking {
        auki_odometry::Tracking::Valid { pose } => Ok((pose.clone(), *at)),
        auki_odometry::Tracking::Lost => {
            anyhow::bail!("Scene transform was revoked at capture time")
        }
    }
}

pub fn resolve(
    cache: &HeldTransforms,
    sensor: &crate::sensor_preview::Sensor,
    row: &serde_json::Value,
    pose: &Contract,
    home_peer: &str,
    view: &serde_json::Value,
    timestamp: u64,
) -> Result<(auki_odometry::Pose, u64)> {
    let frame = view["frame"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Home frame unavailable"))?;
    let catalog: CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
    let candidates = catalog
        .products
        .iter()
        .filter_map(|p| {
            let m = p.metadata.as_ref()?;
            if m.schema != crate::compact_pose::METADATA
                || p.manifest.peer_id != sensor.reference.peer_id
            {
                return None;
            }
            let c: Contract = serde_json::from_value(m.value.clone()).ok()?;
            if validate_chain(&c, pose, &sensor.reference.peer_id, home_peer, frame).is_err() {
                return None;
            }
            let output = catalog
                .components
                .iter()
                .flat_map(|c| c.current_outputs.values())
                .find(|o| o.manifest.reference() == p.manifest.producer)?;
            c.validate_output(&output.manifest).ok()?;
            Some((p.manifest.reference(), c, m.source_sequence))
        })
        .collect::<Vec<_>>();
    ensure!(
        candidates.len() == 1,
        "No unambiguous held transform to this Home frame"
    );
    let (reference, c, advertised_sequence) = &candidates[0];
    crate::product_registry::resolved_contract(&row["registries"], c)?;
    cache.at(reference, c, *advertised_sequence, timestamp)
}

use std::{collections::BTreeMap, sync::{Arc, Mutex}, time::Duration};
use auki_component_protocol::{ComponentProtocolClient, ObservationStart, RemoteObservationEvent};
use tokio_util::sync::CancellationToken;

const MAX_PRODUCTS: usize = 128;
const MAX_STATES: usize = 1024;

/// Home-owned view of SDK retained Products, shared by pose, depth and AR readers.
/// No parallel pose queue, finite network fetch, TTL, or per-reader subscription.
#[derive(Clone, Default)]
pub struct HeldTransforms(Arc<Mutex<BTreeMap<String, Arc<Mutex<HeldState>>>>>);

struct HeldState {
    contract: Contract,
    product: Option<RetainedProduct<Update>>,
    last: Option<(u64, u64)>,
    floor: u64,
    detail: String,
}

impl HeldState {
    fn invalidate(&mut self, detail: impl Into<String>) {
        self.product = None;
        self.last = None;
        self.floor = 0;
        self.detail = detail.into();
    }

    fn accept(&mut self, o: &Observation<Update>) -> Result<()> {
        let product = self.product.as_ref().ok_or_else(|| anyhow::anyhow!("Held stream unavailable"))?;
        ensure!(o.output == product.producer.reference(), "Transform observation provenance mismatch");
        o.payload.validate()?;
        if let Some((sequence, timestamp)) = self.last {
            ensure!(sequence.checked_add(1) == Some(o.sequence) && o.timestamp_ns > timestamp,
                "Non-monotonic or discontinuous held transform stream");
        }
        ensure!(o.sequence >= self.floor, "Transform precedes stream gap");
        self.last = Some((o.sequence, o.timestamp_ns));
        self.detail.clear();
        Ok(())
    }

    fn at(&self, advertised_sequence: u64, timestamp: u64) -> Result<(auki_odometry::Pose, u64)> {
        let product = self.product.as_ref().ok_or_else(|| anyhow::anyhow!("{}", self.detail))?;
        let (last, _) = self.last.ok_or_else(|| anyhow::anyhow!("Held transform synchronizing"))?;
        // Catalog metadata is not transform data, but can tell us the stream is behind.
        ensure!(last >= advertised_sequence, "Held transform has not reached advertised sequence");
        let samples = product.buffer().snapshot(self.floor, last).into_iter()
            .map(|e| (e.payload.timestamp_ns, (*e.payload.payload).clone())).collect::<Vec<_>>();
        held_at(&samples, timestamp)
    }
}

impl HeldTransforms {
    pub fn at(&self, reference: &ProductReference, contract: &Contract,
        advertised_sequence: u64, timestamp: u64) -> Result<(auki_odometry::Pose, u64)> {
        let key = serde_json::to_string(reference)?;
        let entries = self.0.lock().map_err(|_| anyhow::anyhow!("Held cache unavailable"))?;
        let state = entries.get(&key).ok_or_else(|| anyhow::anyhow!("Held transform subscription pending"))?
            .lock().map_err(|_| anyhow::anyhow!("Held stream unavailable"))?;
        ensure!(state.contract == *contract, "Held transform contract changed");
        state.at(advertised_sequence, timestamp)
    }

    /// Discovery owns lifetimes; each worker owns one SDK continuing subscription.
    /// Cancelling/removing a Product invalidates reads before awaited cleanup.
    pub async fn serve(&self, peer: &auki_sdk::AukiPeer,
        mut catalogs: tokio::sync::watch::Receiver<crate::device_catalogs::Catalogs>,
        stop: &CancellationToken) {
        let client = ComponentProtocolClient::new(peer.protocols());
        let lifetime = stop.child_token();
        let _cancel_on_drop = lifetime.clone().drop_guard();
        let mut workers = BTreeMap::<String, (String, CancellationToken, tokio::task::JoinHandle<()>)>::new();
        loop {
            let rows = catalogs.borrow().clone();
            let mut desired = BTreeMap::new();
            for (owner, row) in rows {
                let Ok(catalog) = serde_json::from_value::<CatalogSnapshot>(row["catalog"].clone()) else { continue; };
                for p in &catalog.products {
                    let Some(m) = p.metadata.as_ref().filter(|m| m.schema == crate::compact_pose::METADATA) else { continue; };
                    let Ok(c) = serde_json::from_value::<Contract>(m.value.clone()) else { continue; };
                    if c.lifetime != Lifetime::Held || p.manifest.peer_id != owner { continue; }
                    let Some(o) = catalog.components.iter().flat_map(|c| c.current_outputs.values())
                        .find(|o| o.manifest.reference() == p.manifest.producer) else { continue; };
                    if c.validate_output(&o.manifest).is_err()
                        || crate::product_registry::resolved_contract(&row["registries"], &c).is_err() { continue; }
                    let Ok(peer) = owner.parse() else { continue; };
                    let reference = p.manifest.reference();
                    let key = serde_json::to_string(&reference).unwrap();
                    let fingerprint = serde_json::to_string(&(&c, &row["catalog_route"])).unwrap();
                    let sensor = crate::sensor_preview::Sensor {id:key.clone(), reference, peer,
                        route:row["catalog_route"].as_str().and_then(|s|s.parse().ok()),
                        datatype:crate::compact_pose::SCHEMA.into(), metadata:Some(m.clone()), ar_supported:false};
                    desired.insert(key, (fingerprint, sensor, c));
                }
            }
            let removed = workers.iter().filter(|(key,(fingerprint,_,_))|
                desired.get(*key).is_none_or(|(f,_,_)| f != fingerprint))
                .map(|(key,_)|key.clone()).collect::<Vec<_>>();
            for key in removed {
                self.0.lock().unwrap().remove(&key);
                let (_, cancel, task) = workers.remove(&key).unwrap();
                cancel.cancel();
                let _ = task.await;
            }
            for (key, (fingerprint, sensor, contract)) in desired {
                if workers.contains_key(&key) || workers.len() >= MAX_PRODUCTS { continue; }
                let state = Arc::new(Mutex::new(HeldState {contract, product:None, last:None, floor:0,
                    detail:"Held transform synchronizing".into()}));
                self.0.lock().unwrap().insert(key.clone(), state.clone());
                let cancel = lifetime.child_token();
                let task = tokio::spawn(follow(client.clone(), sensor, state, cancel.clone()));
                workers.insert(key, (fingerprint, cancel, task));
            }
            tokio::select! {
                _ = stop.cancelled() => break,
                changed = catalogs.changed() => if changed.is_err() { break; },
            }
        }
        self.0.lock().unwrap().clear();
        for (_, (_, cancel, task)) in workers {
            cancel.cancel();
            let _ = task.await;
        }
    }
}

async fn follow(client: ComponentProtocolClient, sensor: crate::sensor_preview::Sensor,
    state: Arc<Mutex<HeldState>>, stop: CancellationToken) {
    let mut backoff = 1;
    loop {
        state.lock().unwrap().invalidate("Held transform reconnecting");
        let run = async {
            let limits = BufferLimits {max_entries:Some(MAX_STATES), max_bytes:Some(1024 * 1024), target_duration:None};
            // Stream the retained tail and continue atomically: no fetch/subscribe race.
            let start = ObservationStart::FromSequence {sequence:0};
            let connect = async {
                if let Some(route) = &sensor.route {
                    client.subscribe_product_exact(sensor.peer, route.clone(), sensor.reference.clone(), start, limits, |_: &Update|256).await
                } else {
                    client.subscribe_product(sensor.peer, sensor.reference.clone(), start, limits, |_: &Update|256).await
                }
            };
            let mut sub = tokio::time::timeout(Duration::from_secs(5), connect).await??;
            {
                let mut state = state.lock().unwrap();
                ensure!(sub.product().reference() == sensor.reference
                    && sub.product().manifest.producer == sub.product().producer.reference(), "Held Product changed");
                state.contract.validate_output(&sub.product().producer)?;
                state.product = Some(sub.product().clone());
            }
            let result: Result<()> = async {
                loop {
                    match sub.next().await? {
                        Some(RemoteObservationEvent::Observation(o)) => {
                            state.lock().unwrap().accept(&o)?;
                            backoff = 1;
                        }
                        Some(RemoteObservationEvent::Gap(gap)) => {
                            // Never use the pre-gap state across unknown revocations.
                            let mut state = state.lock().unwrap();
                            state.last = None;
                            state.floor = gap.available_from;
                            state.detail = "Held transform gap; resynchronizing".into();
                        }
                        Some(RemoteObservationEvent::Closed(_)) | None => {
                            state.lock().unwrap().invalidate("Held transform Product ended");
                            return Ok(());
                        }
                    }
                }
            }.await;
            if let Err(error) = &result {
                state.lock().unwrap().invalidate(format!("Held transform stream unavailable: {error}"));
            }
            // Drop is also cancellation-safe; bound graceful close on failed transports.
            let _ = tokio::time::timeout(Duration::from_secs(1), sub.close()).await;
            result
        };
        let result = tokio::select! {_ = stop.cancelled() => break, result = run => result};
        match result {
            Ok(()) => break, // Terminal Products require a new Catalog identity.
            Err(error) => state.lock().unwrap().invalidate(format!("Held transform stream unavailable: {error}")),
        }
        tokio::select! {_ = stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}}
        backoff = (backoff * 2).min(8);
    }
    state.lock().unwrap().invalidate("Held transform subscription stopped");
}
