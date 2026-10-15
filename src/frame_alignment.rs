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
pub struct HeldTransforms(Arc<Mutex<BTreeMap<String, Arc<Mutex<HeldState>>>>>, pub(crate) crate::time_history::History);

struct HeldState {
    contract: Contract,
    product: Option<RetainedProduct<Update>>,
    last: Option<(u64, u64)>,
    stream_last: Option<(u64, u64)>,
    live: bool,
    floor: u64,
    detail: String,
    received: Option<std::time::Instant>,
    arrivals: std::collections::VecDeque<std::time::Instant>,
}

pub struct PoseHistory {
    pub producer: OutputManifest,
    pub latest: Observation<Update>,
    pub samples: Vec<Observation<Update>>,
    pub received: std::time::Instant,
    pub received_hz: f64,
}

impl HeldState {
    fn invalidate(&mut self, detail: impl Into<String>) {
        // A sampled observation remains true at its capture time after a
        // transport interruption. Retain it, but never present it as live.
        // Held state may have been revoked while disconnected: keep that path
        // fail-closed until its retained transition stream is re-established.
        if self.contract.lifetime == Lifetime::Held {
            self.product = None;
            self.last = None;
            self.floor = 0;
            self.received = None;
        }
        self.stream_last = None;
        self.live = false;
        self.arrivals.clear();
        self.detail = detail.into();
    }

    fn accept(&mut self, o: &Observation<Update>) -> Result<()> {
        let product = self.product.as_ref().ok_or_else(|| anyhow::anyhow!("Held stream unavailable"))?;
        ensure!(o.output == product.producer.reference(), "Transform observation provenance mismatch");
        o.payload.validate()?;
        if let Some((sequence, timestamp)) = self.stream_last {
            ensure!(sequence.checked_add(1) == Some(o.sequence) && o.timestamp_ns > timestamp,
                "Non-monotonic or discontinuous held transform stream");
        }
        if let Some((sequence, timestamp)) = self.last {
            ensure!(o.sequence > sequence && o.timestamp_ns > timestamp,
                "Transform source restarted without a new Output identity");
        }
        ensure!(o.sequence >= self.floor, "Transform precedes stream gap");
        if self.contract.lifetime == Lifetime::Sampled {
            product.buffer().append_shared(Arc::new(Envelope::new(o.sequence, o.timestamp_ns, o.clone())))?;
        }
        self.last = Some((o.sequence, o.timestamp_ns));
        self.stream_last = self.last;
        self.live = true;
        let now = std::time::Instant::now();
        self.received = Some(now);
        self.arrivals.push_back(now);
        while self.arrivals.len() > 4096 || self.arrivals.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(2)) {
            self.arrivals.pop_front();
        }
        self.detail.clear();
        Ok(())
    }

    fn at(&self, advertised_sequence: u64, timestamp: u64) -> Result<(auki_odometry::Pose, u64)> {
        let product = self.product.as_ref().ok_or_else(|| anyhow::anyhow!("{}", self.detail))?;
        let (last, _) = self.last.ok_or_else(|| anyhow::anyhow!("Held transform synchronizing"))?;
        // Catalog metadata is not transform data, but can tell us the stream is behind.
        if self.contract.lifetime == Lifetime::Held {
            ensure!(last >= advertised_sequence, "Held transform has not reached advertised sequence");
        }
        match self.contract.lifetime {
            Lifetime::Held => {
                let samples = product.buffer().snapshot(self.floor, last).into_iter()
                    .map(|e| (e.payload.timestamp_ns, (*e.payload.payload).clone())).collect::<Vec<_>>();
                held_at(&samples, timestamp)
            },
            Lifetime::Sampled => {
                let bracket=product.buffer().bracket_time_ns(timestamp)?;
                let mut samples=Vec::new();
                for e in [bracket.before,bracket.after].into_iter().flatten() {
                    let o=&e.payload;
                    if o.sequence<self.floor || o.sequence>last || samples.last().is_some_and(|s:&(u64,u64,Update)|s.0==o.sequence) {continue;}
                    samples.push((o.sequence,o.timestamp_ns,(*o.payload).clone()));
                }
                Ok((crate::transform_chain::sample_at(&samples, Lifetime::Sampled, timestamp)?, timestamp))
            }
        }
    }

    fn at_with_uncertainty(&self, advertised_sequence: u64, timestamp: u64,
        uncertainty: u64) -> Result<(auki_odometry::Pose, u64)> {
        if uncertainty == 0 { return self.at(advertised_sequence, timestamp); }
        let (start, end) = uncertainty_interval(timestamp, uncertainty)?;
        let product = self.product.as_ref().ok_or_else(|| anyhow::anyhow!("{}", self.detail))?;
        let (last, _) = self.last.ok_or_else(|| anyhow::anyhow!("Held transform synchronizing"))?;
        if self.contract.lifetime == Lifetime::Held {
            ensure!(last >= advertised_sequence, "Held transform has not reached advertised sequence");
        }
        let range = if self.contract.lifetime == Lifetime::Held {
            (self.floor, last)
        } else {
            let before = product.buffer().bracket_time_ns(start)?.before
                .ok_or_else(|| anyhow::anyhow!("Waiting for pose before capture uncertainty interval"))?;
            let after = product.buffer().bracket_time_ns(end)?.after
                .ok_or_else(|| anyhow::anyhow!("Waiting for pose after capture uncertainty interval"))?;
            ensure!(before.payload.sequence >= self.floor && after.payload.sequence <= last,
                "Waiting for validated poses across capture uncertainty interval");
            (before.payload.sequence, after.payload.sequence)
        };
        let samples = product.buffer().snapshot(range.0, range.1).into_iter()
            .map(|e| (e.payload.sequence, e.payload.timestamp_ns, (*e.payload.payload).clone()))
            .collect::<Vec<_>>();
        uncertain_at(&samples, self.contract.lifetime, timestamp, uncertainty)
    }
}

fn uncertainty_interval(timestamp: u64, uncertainty: u64) -> Result<(u64, u64)> {
    ensure!(uncertainty <= crate::capture_time::MAX_CAPTURE_MATCH_UNCERTAINTY_NS,
        "Capture-clock conversion uncertainty exceeds 20 ms");
    let start = timestamp.checked_sub(uncertainty)
        .ok_or_else(|| anyhow::anyhow!("Capture uncertainty precedes this clock session"))?;
    let end = timestamp.checked_add(uncertainty)
        .ok_or_else(|| anyhow::anyhow!("Capture uncertainty timestamp overflow"))?;
    Ok((start, end))
}

/// A converted instant denotes an interval, not permission to cross tracking
/// loss or a calibration transition. Held states remain held; they need no
/// heartbeat, but the entire interval must select the same retained observation.
fn uncertain_at(samples: &[(u64, u64, Update)], lifetime: Lifetime, timestamp: u64,
    uncertainty: u64) -> Result<(auki_odometry::Pose, u64)> {
    let (start, end) = uncertainty_interval(timestamp, uncertainty)?;
    match lifetime {
        Lifetime::Held => {
            let held = samples.iter().map(|(_, t, u)| (*t, u.clone())).collect::<Vec<_>>();
            let before = held_at(&held, start)?;
            let after = held_at(&held, end)?;
            ensure!(before.1 == after.1, "Capture uncertainty crosses a held transform transition");
            Ok(before)
        },
        Lifetime::Sampled => {
            crate::transform_chain::sample_at(samples, lifetime, start)?;
            crate::transform_chain::sample_at(samples, lifetime, end)?;
            for (_, _, sample) in samples {
                sample.validate()?;
                ensure!(matches!(sample.tracking, auki_odometry::Tracking::Valid {..}),
                    "Tracking lost inside capture uncertainty interval");
            }
            ensure!(samples.windows(2).all(|w| w[0].0.checked_add(1) == Some(w[1].0)
                && w[1].1 > w[0].1 && w[1].1 - w[0].1 <= crate::transform_chain::GAP_NS),
                "Pose gap inside capture uncertainty interval");
            Ok((crate::transform_chain::sample_at(samples, lifetime, timestamp)?, timestamp))
        },
    }
}

impl HeldTransforms {
    /// The viewer, chat and AR consume the same retained SDK Product. Arrival
    /// statistics are diagnostics only; capture time stays in the Observation.
    pub fn snapshot(&self, reference: &ProductReference, contract: &Contract) -> Result<PoseHistory> {
        let key = serde_json::to_string(reference)?;
        let entries = self.0.lock().map_err(|_| anyhow::anyhow!("Pose history unavailable"))?;
        let state = entries.get(&key).ok_or_else(|| anyhow::anyhow!("Waiting for fresh sensor poses"))?
            .lock().map_err(|_| anyhow::anyhow!("Pose history unavailable"))?;
        ensure!(state.contract == *contract, "Pose history contract changed");
        ensure!(state.live, "Waiting for fresh sensor poses");
        let product = state.product.as_ref().ok_or_else(|| anyhow::anyhow!("Waiting for fresh sensor poses"))?;
        let (last, _) = state.last.ok_or_else(|| anyhow::anyhow!("Waiting for fresh sensor poses"))?;
        let samples = product.buffer().snapshot(state.floor, last).into_iter()
            .map(|e| e.payload.clone()).collect::<Vec<_>>();
        let latest = samples.last().cloned().ok_or_else(|| anyhow::anyhow!("Waiting for fresh sensor poses"))?;
        let received = state.received.ok_or_else(|| anyhow::anyhow!("Waiting for fresh sensor poses"))?;
        let seconds = state.arrivals.front().map(|t| received.duration_since(*t).as_secs_f64()).unwrap_or(0.);
        Ok(PoseHistory {producer:product.producer.clone(), latest, samples, received,
            received_hz:if seconds > 0. {(state.arrivals.len()-1) as f64 / seconds} else {0.}})
    }
    pub fn at(&self, reference: &ProductReference, contract: &Contract,
        advertised_sequence: u64, timestamp: u64) -> Result<(auki_odometry::Pose, u64)> {
        self.at_with_uncertainty(reference, contract, advertised_sequence, timestamp, 0)
    }

    pub fn at_with_uncertainty(&self, reference: &ProductReference, contract: &Contract,
        advertised_sequence: u64, timestamp: u64, uncertainty: u64) -> Result<(auki_odometry::Pose, u64)> {
        let key = serde_json::to_string(reference)?;
        let entries = self.0.lock().map_err(|_| anyhow::anyhow!("Held cache unavailable"))?;
        let state = entries.get(&key).ok_or_else(|| anyhow::anyhow!("Held transform subscription pending"))?
            .lock().map_err(|_| anyhow::anyhow!("Held stream unavailable"))?;
        ensure!(state.contract == *contract, "Held transform contract changed");
        state.at_with_uncertainty(advertised_sequence, timestamp, uncertainty)
    }

    /// Discovery owns lifetimes; each worker owns one SDK continuing subscription.
    /// Cancelling/removing a Product invalidates reads before awaited cleanup.
    pub async fn serve(&self, peer: &auki_sdk::AukiPeer,
        catalogs: tokio::sync::watch::Receiver<crate::device_catalogs::Catalogs>,
        stop: &CancellationToken) {
        tokio::join!(self.serve_poses(peer,catalogs.clone(),stop),self.1.serve(peer,catalogs,stop));
    }

    async fn serve_poses(&self, peer: &auki_sdk::AukiPeer,
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
                if row["catalog_status"] != "ready" { continue; }
                let Ok(catalog) = serde_json::from_value::<CatalogSnapshot>(row["catalog"].clone()) else { continue; };
                for p in &catalog.products {
                    let Some(m) = p.metadata.as_ref().filter(|m| m.schema == crate::compact_pose::METADATA) else { continue; };
                    let Ok(c) = serde_json::from_value::<Contract>(m.value.clone()) else { continue; };
                    if p.manifest.peer_id != owner { continue; }
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
                {
                    let mut entries = self.0.lock().unwrap();
                    let sampled = entries.get(&key).is_some_and(|state| {
                        let mut state = state.lock().unwrap();
                        if state.contract.lifetime != Lifetime::Sampled { return false; }
                        state.invalidate("Sampled pose publisher unavailable");
                        true
                    });
                    if !sampled { entries.remove(&key); }
                }
                let (_, cancel, task) = workers.remove(&key).unwrap();
                cancel.cancel();
                let _ = task.await;
            }
            self.0.lock().unwrap().retain(|key, state| workers.contains_key(key)
                || state.lock().unwrap().received.is_some_and(|at| at.elapsed() < crate::observation_history::WINDOW));
            for (key, (fingerprint, sensor, contract)) in desired {
                if workers.contains_key(&key) || workers.len() >= MAX_PRODUCTS { continue; }
                let state = {
                    let mut entries = self.0.lock().unwrap();
                    if let Some(existing) = entries.get(&key) {
                        if existing.lock().unwrap().contract != contract { continue; }
                        existing.clone()
                    } else {
                        if entries.len() >= MAX_PRODUCTS { continue; }
                        let state = Arc::new(Mutex::new(HeldState {contract, product:None, last:None,
                            stream_last:None,live:false,floor:0,received:None,arrivals:Default::default(),
                            detail:"Held transform synchronizing".into()}));
                        entries.insert(key.clone(),state.clone());
                        state
                    }
                };
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
            // Held state may predate the ten-second window and still be valid.
            // Keep its bounded transition history, including revocations.
            let limits = if state.lock().unwrap().contract.lifetime == Lifetime::Held {
                BufferLimits {max_entries:Some(MAX_STATES), max_bytes:Some(1024 * 1024), target_duration:None}
            } else { crate::observation_history::poses() };
            // Held state needs its retained baseline. A sampled pose stream
            // starts with fresh measurements: replaying an old tail must not
            // make the viewer report a stopped peer as live on reconnect.
            let start = if state.lock().unwrap().contract.lifetime == Lifetime::Held {
                ObservationStart::FromSequence {sequence:0}
            } else { ObservationStart::NewOnly };
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
                if state.contract.lifetime == Lifetime::Held {
                    state.product = Some(sub.product().clone());
                } else if let Some(retained) = &state.product {
                    ensure!(retained.reference() == sub.product().reference()
                        && retained.producer == sub.product().producer, "Sampled pose history source changed");
                } else {
                    state.product = Some(RetainedProduct::imported_buffer(sub.product().manifest.clone(),
                        sub.product().manifest_hash.clone(),sub.product().producer.clone(),
                        crate::observation_history::poses(),|_: &Update|256)?);
                }
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
                            if state.contract.lifetime == Lifetime::Held {
                                state.last = None;
                                state.floor = gap.available_from;
                            }
                            state.stream_last = None;
                            state.live = false;
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

#[cfg(test)]
mod uncertainty_tests {
    use super::*;
    fn valid(x: f64) -> Update {
        Update {tracking:auki_odometry::Tracking::Valid {pose:auki_odometry::Pose {
            translation:[x,0.,0.],rotation_xyzw:[0.,0.,0.,1.],
        }}}
    }
    #[test]
    fn held_capture_interval_cannot_cross_a_transition_or_revocation() -> Result<()> {
        let samples = [(0,10,valid(1.)),(1,100,valid(1.))];
        assert_eq!(uncertain_at(&samples,Lifetime::Held,50,20)?.1,10);
        // Even identical transform values do not make two calibration epochs
        // interchangeable; this interval could refer to either observation.
        assert!(uncertain_at(&samples,Lifetime::Held,99,2).is_err());
        assert_eq!(uncertain_at(&samples,Lifetime::Held,150,20)?.1,100);
        let samples = [(0,10,valid(1.)),(1,100,Update {tracking:auki_odometry::Tracking::Lost}),
            (2,110,valid(1.))];
        assert!(uncertain_at(&samples,Lifetime::Held,95,5).is_err());
        assert!(uncertain_at(&samples,Lifetime::Held,110,15).is_err());
        assert!(uncertain_at(&samples,Lifetime::Held,10,1).is_err());
        // A held baseline remains valid without artificial periodic updates.
        assert_eq!(uncertain_at(&samples,Lifetime::Held,10_000,20)?.1,110);
        Ok(())
    }
    #[test]
    fn sampled_capture_interval_requires_both_bounds_and_no_internal_loss() -> Result<()> {
        let samples = [(0,100,valid(0.)),(1,150,valid(1.)),(2,200,valid(2.))];
        assert_eq!(uncertain_at(&samples,Lifetime::Sampled,150,20)?.0.translation,[1.,0.,0.]);
        assert!(uncertain_at(&samples,Lifetime::Sampled,110,20).is_err());
        assert!(uncertain_at(&samples,Lifetime::Sampled,190,20).is_err());
        let lost = [(0,100,valid(0.)),(1,150,Update {tracking:auki_odometry::Tracking::Lost}),
            (2,200,valid(2.))];
        // Both endpoints themselves are valid; the interior lost sample still
        // makes the capture uncertain, rather than allowing a false pose.
        assert!(uncertain_at(&lost,Lifetime::Sampled,150,50).is_err());
        let gap = [(0,100,valid(0.)),(2,150,valid(1.)),(3,200,valid(2.))];
        assert!(uncertain_at(&gap,Lifetime::Sampled,150,50).is_err());
        Ok(())
    }
    #[test]
    fn uncertainty_bounds_never_saturate_or_exceed_the_match_budget() {
        assert!(uncertainty_interval(10,11).is_err());
        assert!(uncertainty_interval(u64::MAX,1).is_err());
        assert!(uncertainty_interval(100_000_000,crate::capture_time::MAX_CAPTURE_MATCH_UNCERTAINTY_NS+1).is_err());
    }

    fn retained_state(lifetime: Lifetime) -> HeldState {
        let from = auki_registry::FrameRegistryEntry::in_convention("peer","base",auki_registry::CoordinateConvention::OpenGl);
        let to = auki_registry::FrameRegistryEntry::in_convention("peer","world",auki_registry::CoordinateConvention::OpenGl);
        let clock = auki_time::SessionClock::new("peer","session","capture");
        let contract = Contract {from_frame:(&from).into(),to_frame:(&to).into(),
            clock:crate::model::clock_ref(&clock.registry_entry()),session_id:"session".into(),lifetime};
        let runtime = ComponentRuntime::new("peer");
        let component = runtime.component(ComponentSpec::new("poses").observable(ObservableContract {
            name:"pose".into(),datatype:crate::compact_pose::SCHEMA.into(),schema:crate::compact_pose::SCHEMA.into(),
            access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster,
        })).unwrap();
        let output: ConfiguredObservable<Update> = component.configured_observable(ConfiguredObservableSpec::new("pose",
            contract.output_id(),contract.clock.clone(),PayloadContract::Structured(StructuredPayloadContract {
                modality:"pose".into(),datatype:crate::compact_pose::SCHEMA.into(),schema:crate::compact_pose::SCHEMA.into(),
                observes:"base_in_world".into(),unit:None,
            })).in_registered_frame(to)).unwrap();
        component.expose().unwrap();
        let capture = runtime.capture_buffer("poses",&output,crate::observation_history::poses(),|_: &Update|256).unwrap();
        let source = capture.product();
        let retained = RetainedProduct::imported_buffer(source.manifest.clone(),source.manifest_hash.clone(),source.producer.clone(),
            crate::observation_history::poses(),|_: &Update|256).unwrap();
        HeldState {contract,product:Some(retained),last:None,stream_last:None,live:false,floor:0,
            detail:String::new(),received:None,arrivals:Default::default()}
    }

    fn observation(state: &HeldState, sequence: u64, timestamp: u64) -> Observation<Update> {
        Observation {output:state.product.as_ref().unwrap().producer.reference(),sequence,timestamp_ns:timestamp,
            payload:Arc::new(valid(timestamp as f64))}
    }

    #[test]
    fn sampled_history_survives_disconnect_without_reviving_live_view_or_bridging_a_gap() -> Result<()> {
        let mut state = retained_state(Lifetime::Sampled);
        let first = observation(&state,0,100);
        state.accept(&first)?;
        let second = observation(&state,1,200);
        state.accept(&second)?;
        let reference = state.product.as_ref().unwrap().reference();
        let contract = state.contract.clone();
        state.invalidate("transport disconnected");
        assert!(!state.live);
        // Newer advertised sequence does not invalidate historical brackets.
        assert_eq!(state.at(99,150)?.0.translation[0],150.);
        assert_eq!(state.at_with_uncertainty(99,150,20)?.0.translation[0],150.);
        let cache = HeldTransforms::default();
        let state = Arc::new(Mutex::new(state));
        cache.0.lock().unwrap().insert(serde_json::to_string(&reference)?,state.clone());
        assert!(cache.snapshot(&reference,&contract).is_err(),"retained samples are not live peer evidence");
        {
            let mut state = state.lock().unwrap();
            // NewOnly reconnect skips observations 2 and 3. Keep both valid
            // intervals, but source sequence continuity forbids joining them.
            let next = observation(&state,4,400);
            state.accept(&next)?;
            let next = observation(&state,5,500);
            state.accept(&next)?;
            assert_eq!(state.at(99,150)?.0.translation[0],150.);
            assert!(state.at(99,300).is_err());
            assert_eq!(state.at(99,450)?.0.translation[0],450.);
            assert!(state.at(99,501).is_err(),"never extrapolate");
            let reset = observation(&state,0,600);
            assert!(state.accept(&reset).is_err(),"source reset requires a new identity");
        }
        assert_eq!(cache.snapshot(&reference,&contract)?.latest.sequence,5);
        Ok(())
    }

    #[test]
    fn held_calibration_remains_unavailable_during_unknown_disconnect() -> Result<()> {
        let mut state = retained_state(Lifetime::Held);
        let sample = observation(&state,0,100);
        state.product.as_ref().unwrap().buffer().append_shared(Arc::new(Envelope::new(0,100,sample.clone())))?;
        state.accept(&sample)?;
        assert!(state.at(0,150).is_ok());
        state.invalidate("transport disconnected");
        assert!(state.at(0,150).is_err());
        assert!(state.product.is_none());
        assert!(!state.live);
        Ok(())
    }
}
