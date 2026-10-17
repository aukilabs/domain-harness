//! Home's follower for declared robot bodies: one continuing SDK subscription
//! per robot-state Product in the discovered Catalogs, latest fresh observation
//! cached for the viewer. Ages are Home receipt ages, so no clocks are compared.
//! Joints and health arrive one capture at a time (one arm per observation), so
//! they are merged by joint name, each reading kept only while it is fresh.
use crate::robot_state::{BATTERY_SCHEMA, Battery, BodyContract, HEALTH_SCHEMA, Health, JOINTS_SCHEMA, Joints, METADATA, POINTING_SCHEMA, Pointing};
use anyhow::{Result, ensure};
use auki_component_protocol::{ComponentProtocolClient, ObservationStart, RemoteObservationEvent};
use auki_components::*;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const MAX_PRODUCTS: usize = 64;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Joints,
    Health,
    Battery,
    Pointing,
}
impl Kind {
    fn of(schema: &str) -> Option<Self> {
        match schema {
            JOINTS_SCHEMA => Some(Self::Joints),
            HEALTH_SCHEMA => Some(Self::Health),
            BATTERY_SCHEMA => Some(Self::Battery),
            POINTING_SCHEMA => Some(Self::Pointing),
            _ => None,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Joints => "joints",
            Self::Health => "health",
            Self::Battery => "battery",
            Self::Pointing => "pointing",
        }
    }
    /// The field merged by key across observations, and how long a reading
    /// stays without a newer one (the viewer's own staleness limits).
    fn merged(self) -> Option<(&'static str, Duration)> {
        match self {
            Self::Joints => Some(("positions", Duration::from_secs(3))),
            Self::Health => Some(("joints", Duration::from_secs(5))),
            Self::Battery | Self::Pointing => None,
        }
    }
}

struct Latest {
    sequence: u64,
    timestamp_ns: u64,
    payload: Value,
    received: Instant,
}
struct Entry {
    owner: String,
    kind: Kind,
    body: BodyContract,
    product: ProductReference,
    output: OutputReference,
    latest: Option<Latest>,
    /// For merged kinds: (field, key) → (reading, when Home received it). An
    /// empty key holds a whole field, such as an older publisher's battery.
    parts: BTreeMap<(String, String), (Value, Instant)>,
}
impl Entry {
    fn state(&self, latest: &Latest) -> Value {
        let Some((_, keep)) = self.kind.merged() else { return latest.payload.clone() };
        let mut state = serde_json::Map::new();
        for ((field, key), (value, received)) in &self.parts {
            if received.elapsed() > keep { continue; }
            if key.is_empty() {
                state.insert(field.clone(), value.clone());
            } else {
                let map = state.entry(field.clone()).or_insert_with(|| json!({}));
                map[key.as_str()] = value.clone();
            }
        }
        Value::Object(state)
    }
}

#[derive(Clone, Default)]
pub struct RobotStates(Arc<Mutex<BTreeMap<String, Entry>>>);

/// The declared body, if this Product's producing Output matches it exactly.
fn declared(catalog: &CatalogSnapshot, owner: &str, p: &CatalogProductEntry) -> Option<(Kind, BodyContract)> {
    let m = p.metadata.as_ref().filter(|m| m.schema == METADATA)?;
    let body: BodyContract = serde_json::from_value(m.value.clone()).ok()?;
    let output = catalog.components.iter().flat_map(|c| c.current_outputs.values())
        .find(|o| o.manifest.reference() == p.manifest.producer)?;
    let PayloadContract::Structured(payload) = &output.manifest.payload else { return None };
    let kind = Kind::of(&payload.datatype)?;
    (body.validate().is_ok() && p.manifest.peer_id == owner && body.base_frame.peer_id == owner
        && payload.schema == payload.datatype
        && output.manifest.output_id == body.output_id(kind.name())
        && output.manifest.clock == body.clock).then_some((kind, body))
}

impl RobotStates {
    /// Every declared body with its latest joints and health, grouped by owner and body.
    pub fn view(&self) -> Value {
        let entries = self.0.lock().unwrap();
        let mut robots = BTreeMap::<String, Value>::new();
        for e in entries.values() {
            let key = serde_json::to_string(&(&e.owner, &e.body)).unwrap();
            let robot = robots.entry(key).or_insert_with(|| json!({"peer_id":e.owner,"model":e.body.model,
                "base_frame":e.body.base_frame,"session_id":e.body.session_id,"joints":null,"health":null,"battery":null,"pointing":null}));
            if let Some(l) = &e.latest {
                // timestamp_ns and age_ms are the newest observation's.
                robot[e.kind.name()] = json!({"sequence":l.sequence,"timestamp_ns":l.timestamp_ns.to_string(),
                    "clock":e.body.clock,"source":{"product":e.product,"output":e.output},
                    "age_ms":l.received.elapsed().as_millis() as u64,"state":e.state(l)});
            }
        }
        json!({"robots":robots.into_values().collect::<Vec<_>>()})
    }

    /// Discovery owns lifetimes: Products that disappear or change are dropped.
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
                let Ok(catalog) = serde_json::from_value::<CatalogSnapshot>(row["catalog"].clone()) else { continue };
                let Ok(peer) = owner.parse() else { continue };
                for p in &catalog.products {
                    let Some((kind, body)) = declared(&catalog, &owner, p) else { continue };
                    let reference = p.manifest.reference();
                    let key = serde_json::to_string(&reference).unwrap();
                    let fingerprint = serde_json::to_string(&(&body, &row["catalog_route"])).unwrap();
                    let sensor = crate::sensor_preview::Sensor {id:key.clone(), reference:reference.clone(), peer,
                        route:row["catalog_route"].as_str().and_then(|s| s.parse().ok()),
                        datatype:kind.name().into(), metadata:p.metadata.clone(), ar_supported:false};
                    desired.insert(key, (fingerprint, sensor, Entry {owner:owner.clone(), kind, body,
                        product:reference,output:p.manifest.producer.clone(),latest:None,parts:BTreeMap::new()}));
                }
            }
            let removed = workers.iter().filter(|(key, (fingerprint, _, _))|
                desired.get(*key).is_none_or(|(f, _, _)| f != fingerprint))
                .map(|(key, _)| key.clone()).collect::<Vec<_>>();
            for key in removed {
                self.0.lock().unwrap().remove(&key);
                let (_, cancel, task) = workers.remove(&key).unwrap();
                cancel.cancel();
                let _ = task.await;
            }
            for (key, (fingerprint, sensor, entry)) in desired {
                if workers.contains_key(&key) || workers.len() >= MAX_PRODUCTS { continue; }
                let kind = entry.kind;
                self.0.lock().unwrap().insert(key.clone(), entry);
                let cancel = lifetime.child_token();
                let task = match kind {
                    Kind::Joints => tokio::spawn(follow::<Joints>(client.clone(), sensor, self.clone(), key.clone(), cancel.clone(), Joints::validate)),
                    Kind::Health => tokio::spawn(follow::<Health>(client.clone(), sensor, self.clone(), key.clone(), cancel.clone(), Health::validate)),
                    Kind::Battery => tokio::spawn(follow::<Battery>(client.clone(), sensor, self.clone(), key.clone(), cancel.clone(), Battery::validate)),
                    Kind::Pointing => tokio::spawn(follow::<Pointing>(client.clone(), sensor, self.clone(), key.clone(), cancel.clone(), Pointing::validate)),
                };
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

    fn accept(&self, key: &str, output: &OutputReference, clock: &ClockReference,
        sequence: u64, timestamp_ns: u64, payload: Value) -> Result<()> {
        if let Some(e) = self.0.lock().unwrap().get_mut(key) {
            ensure!(output == &e.output && clock == &e.body.clock, "Robot state source or capture clock changed");
            // Never step backwards within one Output.
            if e.latest.as_ref().is_some_and(|l| sequence <= l.sequence) { return Ok(()); }
            let received = Instant::now();
            if let (Some((keyed, _)), Value::Object(fields)) = (e.kind.merged(), &payload) {
                for (field, value) in fields {
                    match value {
                        Value::Object(map) if field == keyed => for (key, reading) in map {
                            e.parts.insert((field.clone(), key.clone()), (reading.clone(), received));
                        },
                        Value::Null => {}
                        _ => { e.parts.insert((field.clone(), String::new()), (value.clone(), received)); }
                    }
                }
                // Bound the merge to what is still fresh.
                if let Some((_, keep)) = e.kind.merged() { e.parts.retain(|_, (_, at)| at.elapsed() <= keep); }
            }
            e.latest = Some(Latest {sequence, timestamp_ns, payload, received});
        }
        Ok(())
    }
}

async fn follow<T>(client: ComponentProtocolClient, sensor: crate::sensor_preview::Sensor,
    states: RobotStates, key: String, stop: CancellationToken, validate: fn(&T) -> Result<()>)
where T: ContractType + serde::de::DeserializeOwned + serde::Serialize + Send + Sync + 'static {
    let mut backoff = 1;
    loop {
        let run = async {
            // Only fresh state: a retained tail would look live after the robot stopped.
            let limits = crate::observation_history::structured();
            let size = |v: &T| serde_json::to_vec(v).map_or(usize::MAX, |b| b.len());
            let connect = async {
                if let Some(route) = &sensor.route {
                    client.subscribe_product_exact(sensor.peer, route.clone(), sensor.reference.clone(), ObservationStart::NewOnly, limits, size).await
                } else {
                    client.subscribe_product(sensor.peer, sensor.reference.clone(), ObservationStart::NewOnly, limits, size).await
                }
            };
            let mut sub = tokio::time::timeout(Duration::from_secs(5), connect).await??;
            ensure!(sub.product().reference() == sensor.reference
                && sub.product().manifest.producer == sub.product().producer.reference(), "Robot state Product changed");
            let result: Result<()> = async {
                loop {
                    match sub.next().await? {
                        Some(RemoteObservationEvent::Observation(o)) => {
                            ensure!(o.output == sub.product().producer.reference(), "Robot state provenance mismatch");
                            validate(&o.payload)?;
                            states.accept(&key, &o.output, &sub.product().producer.clock,
                                o.sequence, o.timestamp_ns, serde_json::to_value(&*o.payload)?)?;
                            backoff = 1;
                        }
                        Some(RemoteObservationEvent::Gap(_)) => {}
                        Some(RemoteObservationEvent::Closed(_)) | None => return Ok(()),
                    }
                }
            }.await;
            let _ = tokio::time::timeout(Duration::from_secs(1), sub.close()).await;
            result
        };
        tokio::select! {
            _ = stop.cancelled() => return,
            result = run => if let Err(e) = result { eprintln!("Robot state stream unavailable: {e}"); },
        }
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {},
        }
        backoff = (backoff * 2).min(10);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry {owner:"robot".into(),kind:Kind::Health,
            body:BodyContract {model:"bracketbot".into(),session_id:"session".into(),
                base_frame:crate::compact_pose::FrameRef {peer_id:"robot".into(),id:"base".into(),hash:"frame-hash".into()},
                clock:ClockReference {peer_id:"robot".into(),id:"session-clock".into(),hash:"clock-hash".into()}},
            product:ProductReference {peer_id:"robot".into(),product_id:"health".into(),manifest_hash:"product-hash".into()},
            output:OutputReference {peer_id:"robot".into(),component_id:"body".into(),component_manifest_hash:"component-hash".into(),
                slot:"health".into(),output_id:"health-output".into(),manifest_hash:"output-hash".into()},latest:None}
    }

    #[test]
    fn view_preserves_each_states_capture_clock_and_source() {
        let e = entry();
        let clock = e.body.clock.clone();
        let output = e.output.clone();
        let product = e.product.clone();
        let states = RobotStates::default();
        states.0.lock().unwrap().insert("health".into(), e);
        let capture = 9_007_199_254_740_993;
        states.accept("health", &output, &clock, 7, capture, json!({"battery":{"percent":50}})).unwrap();
        let view = states.view();
        let health = &view["robots"][0]["health"];
        assert_eq!(health["timestamp_ns"], capture.to_string(), "capture nanoseconds stay exact for browser readers");
        assert_eq!(health["clock"], json!(clock));
        assert_eq!(health["sequence"], 7);
        assert_eq!(health["source"], json!({"product":product,"output":output}));
        states.accept("health", &output, &clock, 6, capture + 1, json!({})).unwrap();
        assert_eq!(states.view()["robots"][0]["health"]["sequence"], 7);
    }

    #[test]
    fn changed_clock_or_source_cannot_be_relabelled_as_the_declared_state() {
        let e = entry();
        let mut clock = e.body.clock.clone();
        let mut output = e.output.clone();
        let states = RobotStates::default();
        states.0.lock().unwrap().insert("health".into(), e);
        clock.hash = "other-clock-definition".into();
        assert!(states.accept("health", &output, &clock, 7, 10, json!({})).is_err());
        clock.hash = "clock-hash".into();
        output.manifest_hash = "other-output".into();
        assert!(states.accept("health", &output, &clock, 7, 10, json!({})).is_err());
        assert!(states.view()["robots"][0]["health"].is_null());
    }
}
