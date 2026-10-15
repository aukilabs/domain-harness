//! One SDK subscription per discovered time-transform Product, shared by readers.
use crate::capture_time::{Contract, Sample};
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{ComponentProtocolClient, ObservationStart, RemoteObservationEvent};
use auki_components::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct History(Arc<Mutex<BTreeMap<String, Arc<Mutex<Entry>>>>>);
const MAX_CLOCK_IDENTITIES: usize = 4096;

/// An immutable source binding survives transport and Catalog churn. It is
/// retained for this Home lifetime, not treated as evidence of peer liveness.
struct Entry {
    contract: Contract,
    reference: ProductReference,
    output: OutputReference,
    anchor: Option<RetainedProduct<Sample>>,
    fault: Option<String>,
}

impl Entry {
    fn reject(&mut self, detail: &str) -> anyhow::Error {
        self.fault.get_or_insert_with(|| detail.into());
        anyhow!("{}", self.fault.as_ref().unwrap())
    }

    fn check(&self) -> Result<()> {
        if let Some(detail) = &self.fault { anyhow::bail!("{detail}"); }
        Ok(())
    }

    fn accept_anchor(&mut self, product: &RetainedProduct<Sample>, observation: &Observation<Sample>) -> Result<()> {
        self.check()?;
        if product.reference() != self.reference || product.producer.reference() != self.output
            || observation.output != self.output || observation.sequence != 0 {
            return Err(self.reject("Session UTC source changed; new clock identity required"));
        }
        self.contract.validate(&product.producer)?;
        if let Some(retained) = &self.anchor {
            let records = retained.buffer().snapshot(0, 0);
            let known = &records.first().ok_or_else(|| anyhow!("Retained UTC anchor unavailable"))?.payload;
            if known.timestamp_ns != observation.timestamp_ns || known.payload != observation.payload {
                return Err(self.reject("Session UTC anchor changed; new clock identity required"));
            }
            return Ok(());
        }
        // Do not share the transport's mutable one-entry buffer: next() retains
        // an event before validation, including an invalid replacement anchor.
        let retained = RetainedProduct::imported_buffer(product.manifest.clone(), product.manifest_hash.clone(),
            product.producer.clone(), BufferLimits {max_entries:Some(1),max_bytes:Some(16384),target_duration:None},
            |_: &Sample| 16)?;
        retained.buffer().append_shared(Arc::new(Envelope::new(observation.sequence, observation.timestamp_ns, observation.clone())))?;
        self.anchor = Some(retained);
        Ok(())
    }
}

impl History {
    fn register(&self, contract: Contract, product: &ProductManifest, output: &OutputManifest) -> Result<Arc<Mutex<Entry>>> {
        contract.validate(output)?;
        ensure!(product.producer == output.reference(), "Time Product producer mismatch");
        let key = serde_json::to_string(&crate::model::clock_ref(&contract.from_clock))?;
        let mut entries = self.0.lock().map_err(|_| anyhow!("Time history unavailable"))?;
        if let Some(entry) = entries.get(&key) {
            let mut existing = entry.lock().map_err(|_| anyhow!("Time history unavailable"))?;
            existing.check()?;
            if existing.contract != contract || existing.reference != product.reference() || existing.output != output.reference() {
                return Err(existing.reject("Ambiguous or changed UTC-transform source for the same clock identity"));
            }
            return Ok(entry.clone());
        }
        ensure!(entries.len() < MAX_CLOCK_IDENTITIES, "Session UTC clock identity limit reached (4096); retained anchors were not overwritten");
        let entry = Arc::new(Mutex::new(Entry {contract,reference:product.reference(),output:output.reference(),anchor:None,fault:None}));
        entries.insert(key, entry.clone());
        Ok(entry)
    }

    fn samples(&self, clock: &ClockReference) -> Result<(Contract, Vec<(u64, Sample)>)> {
        let key = serde_json::to_string(clock)?;
        let entries = self
            .0
            .lock()
            .map_err(|_| anyhow!("Time history unavailable"))?;
        let entry = entries.get(&key).ok_or_else(|| anyhow!("No declared time transform for capture clock"))?
            .lock().map_err(|_| anyhow!("Time history unavailable"))?;
        entry.check()?;
        let p = entry.anchor.as_ref().ok_or_else(|| anyhow!("Time-transform subscription synchronizing"))?;
        let samples = p.buffer().snapshot(0, 0).iter()
            .map(|o| (o.payload.timestamp_ns, (*o.payload.payload).clone())).collect();
        Ok((entry.contract.clone(), samples))
    }

    pub fn convert(
        &self,
        from: &ClockReference,
        to: &ClockReference,
        timestamp: u64,
    ) -> Result<(u64, u64)> {
        if from == to {
            return Ok((timestamp, 0));
        }
        let (a, sa) = self.samples(from)?;
        let (b, sb) = self.samples(to)?;
        let cross_host_error = if a.to_clock == b.to_clock {
            0
        } else {
            // UTC has a standard epoch but two local realtime readers are not
            // inherently synchronized. Only compose with declared error bounds.
            let (Some(a), Some(b)) = (a.utc_accuracy_ns, b.utc_accuracy_ns) else {
                anyhow::bail!(
                    "No common registered UTC clock; device wall clocks are not synchronization"
                );
            };
            a.checked_add(b)
                .ok_or_else(|| anyhow!("Time uncertainty overflow"))?
        };
        let (utc, uncertainty) = crate::capture_time::to_utc(&a, &sa, timestamp)?;
        let (candidate, other) = crate::capture_time::from_utc(&b, &sb, utc)?;
        Ok((
            candidate,
            uncertainty
                .checked_add(other)
                .and_then(|v| v.checked_add(cross_host_error))
                .and_then(|v| v.checked_add(a.capture_conversion_uncertainty_ns))
                .and_then(|v| v.checked_add(b.capture_conversion_uncertainty_ns))
                .ok_or_else(|| anyhow!("Time uncertainty overflow"))?,
        ))
    }

    pub fn describe(&self, clock: &ClockReference, timestamp: u64) -> serde_json::Value {
        let result = (|| -> Result<serde_json::Value> {
            let (c, samples) = self.samples(clock)?;
            let (utc, uncertainty) = crate::capture_time::to_utc(&c, &samples, timestamp)?;
            Ok(
                serde_json::json!({"status":"resolved","timestamp_ns":utc.to_string(),
                "clock":crate::model::clock_ref(&c.to_clock),"conversion_uncertainty_ns":uncertainty,
                "utc_accuracy_ns":c.utc_accuracy_ns,"source_clock":clock,
                "capture_conversion_uncertainty_ns":c.capture_conversion_uncertainty_ns}),
            )
        })();
        result.unwrap_or_else(|e|serde_json::json!({"status":"unavailable","detail":e.to_string(),"source_clock":clock}))
    }

    pub async fn serve(
        &self,
        peer: &auki_sdk::AukiPeer,
        mut catalogs: tokio::sync::watch::Receiver<crate::device_catalogs::Catalogs>,
        stop: &CancellationToken,
    ) {
        let client = ComponentProtocolClient::new(peer.protocols());
        let lifetime = stop.child_token();
        let _guard = lifetime.clone().drop_guard();
        let mut workers =
            BTreeMap::<String, (String, CancellationToken, tokio::task::JoinHandle<()>)>::new();
        loop {
            let mut desired = BTreeMap::new();
            for (owner, row) in catalogs.borrow().iter() {
                if row["catalog_status"] != "ready" {
                    continue;
                }
                let Ok(catalog) = serde_json::from_value::<CatalogSnapshot>(row["catalog"].clone())
                else {
                    continue;
                };
                let Ok(peer) = owner.parse() else { continue };
                let Some(route) = row["catalog_route"]
                    .as_str()
                    .and_then(|v| v.parse::<auki_sdk::Multiaddr>().ok())
                else {
                    continue;
                };
                for p in &catalog.products {
                    let Some(m) = p
                        .metadata
                        .as_ref()
                        .filter(|m| m.schema == crate::capture_time::METADATA)
                    else {
                        continue;
                    };
                    let Ok(contract) = serde_json::from_value::<Contract>(m.value.clone()) else {
                        continue;
                    };
                    let Some(o) = catalog
                        .components
                        .iter()
                        .flat_map(|c| c.current_outputs.values())
                        .find(|o| o.manifest.reference() == p.manifest.producer)
                    else {
                        continue;
                    };
                    if p.manifest.peer_id != *owner
                        || p.manifest.hash() != p.manifest_hash
                        || contract.validate(&o.manifest).is_err()
                        || crate::product_registry::resolved_time_contract(
                            &row["registries"],
                            &contract,
                        )
                        .is_err()
                    {
                        continue;
                    }
                    let entry = match self.register(contract.clone(), &p.manifest, &o.manifest) {
                        Ok(entry) => entry,
                        Err(error) => {
                            eprintln!("Time-transform declaration rejected: {error}");
                            continue;
                        }
                    };
                    let reference = p.manifest.reference();
                    let key = serde_json::to_string(&reference).unwrap();
                    let fingerprint =
                        serde_json::to_string(&(&contract, &row["catalog_route"])).unwrap();
                    let sensor = crate::sensor_preview::Sensor {
                        peer,
                        route: Some(route.clone()),
                        id: p.manifest.product_id.clone(),
                        reference,
                        datatype: crate::capture_time::SCHEMA.into(),
                        metadata: Some(m.clone()),
                        ar_supported: false,
                    };
                    desired.insert(key, (fingerprint, sensor, entry));
                }
            }
            let removed = workers
                .iter()
                .filter(|(key, (f, _, _))| desired.get(*key).is_none_or(|(next, _, _)| next != f))
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in removed {
                let (_, cancel, task) = workers.remove(&key).unwrap();
                cancel.cancel();
                let _ = task.await;
            }
            for (key, (fingerprint, sensor, entry)) in desired {
                if workers.contains_key(&key) || workers.len() >= 128 {
                    continue;
                }
                let cancel = lifetime.child_token();
                let task = tokio::spawn(follow(client.clone(), sensor, entry, cancel.clone()));
                workers.insert(key, (fingerprint, cancel, task));
            }
            tokio::select! {_ = stop.cancelled()=>break, changed=catalogs.changed()=>if changed.is_err(){break;}}
        }
        self.0.lock().unwrap().clear();
        for (_, (_, cancel, task)) in workers {
            cancel.cancel();
            let _ = task.await;
        }
    }
}

async fn follow(
    client: ComponentProtocolClient,
    sensor: crate::sensor_preview::Sensor,
    entry: Arc<Mutex<Entry>>,
    stop: CancellationToken,
) {
    let mut retry = 1;
    loop {
        let run = async {
            let mut sub = tokio::time::timeout(
                Duration::from_secs(5),
                client.subscribe_product_exact(
                    sensor.peer,
                    sensor.route.clone().unwrap(),
                    sensor.reference.clone(),
                    ObservationStart::FromSequence { sequence: 0 },
                    BufferLimits {
                        max_entries: Some(1),
                        max_bytes: Some(16384),
                        target_duration: None,
                    },
                    |_: &Sample| 16,
                ),
            )
            .await??;
            ensure!(
                sub.product().reference() == sensor.reference,
                "Time Product changed"
            );
            entry
                .lock()
                .unwrap()
                .contract
                .validate(&sub.product().producer)?;
            while let Some(event) = sub.next().await? {
                match event {
                    RemoteObservationEvent::Observation(o) => {
                        let mut e = entry.lock().unwrap();
                        e.accept_anchor(sub.product(), &o)?;
                        retry = 1;
                    }
                    RemoteObservationEvent::Gap(_) => {
                        return Err(entry.lock().unwrap().reject("Session UTC source omitted its immutable sequence-zero anchor"));
                    }
                    RemoteObservationEvent::Closed(_) => break,
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(1), sub.close()).await;
            Ok::<(), anyhow::Error>(())
        };
        let result = tokio::select! {_ = stop.cancelled()=>break,result=run=>result};
        {
            let e = entry.lock().unwrap();
            if let Some(detail) = &e.fault {
                eprintln!("Time-transform source rejected: {detail}");
                break;
            }
            // A terminal source cannot alter its held session anchor. If no
            // anchor arrived, retry rather than leaving a permanent cold entry.
            if result.is_ok() && e.anchor.is_some() { break; }
        }
        tokio::select! {_ = stop.cancelled()=>break,_ = tokio::time::sleep(Duration::from_secs(retry))=>{}}
        retry = (retry * 2).min(8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn insert(
        history: &History,
        owner: &str,
        session: &str,
        name: &str,
        offset: i64,
        accuracy: Option<u64>,
    ) -> ClockReference {
        let clock = auki_time::SessionClock::new(owner, session, name).registry_entry();
        let reference = crate::model::clock_ref(&clock);
        let mut c = Contract::local(clock);
        c.utc_accuracy_ns = accuracy;
        let runtime = ComponentRuntime::new(owner);
        let component = runtime
            .component(ComponentSpec::new(name).observable(ObservableContract {
                name: "time".into(),
                datatype: Sample::DATATYPE.into(),
                schema: Sample::DATATYPE.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }))
            .unwrap();
        let output = component
            .configured_observable(ConfiguredObservableSpec::new(
                "time",
                c.output_id(),
                reference.clone(),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "time_transform".into(),
                    datatype: Sample::DATATYPE.into(),
                    schema: Sample::DATATYPE.into(),
                    observes: "test".into(),
                    unit: None,
                }),
            ))
            .unwrap();
        c.validate(&output.manifest()).unwrap();
        component.expose().unwrap();
        let capture = runtime
            .capture_buffer(
                name,
                &output,
                crate::observation_history::structured(),
                |_: &Sample| 16,
            )
            .unwrap();
        for timestamp in [100] {
            output
                .publish(
                    timestamp,
                    Arc::new(Sample {
                        offset_ns: offset,
                        uncertainty_ns: 20,
                    }),
                )
                .unwrap();
        }
        let product = capture.product();
        let entry = history.register(c, &product.manifest, &product.producer).unwrap();
        let observation = product.buffer().snapshot(0, 0)[0].payload.clone();
        entry.lock().unwrap().accept_anchor(&product, &observation).unwrap();
        reference
    }
    #[test]
    fn one_session_resolves_different_capture_clocks_through_declared_utc() {
        let history = History::default();
        let a = insert(&history, "peer", "session", "image", 1000, None);
        let b = insert(&history, "peer", "session", "pose", 1010, None);
        assert_eq!(history.convert(&a, &b, 150).unwrap(), (140, 12_000_040));
        assert_eq!(history.convert(&a, &a, 150).unwrap(), (150, 0));
        assert!(
            history.convert(&a, &b, 9).is_err(),
            "Conversion cannot precede target session zero"
        );
        assert_eq!(history.describe(&a, 150)["timestamp_ns"], "1150");
        let mut reset = a.clone();
        reset.hash = "f".repeat(64);
        assert!(history.convert(&reset, &b, 150).is_err());
    }
    #[test]
    fn different_hosts_require_known_utc_accuracy_and_accumulate_it() {
        let history = History::default();
        let a = insert(&history, "a", "s", "capture", 1000, None);
        let b = insert(&history, "b", "s", "capture", 1000, None);
        assert!(
            history
                .convert(&a, &b, 150)
                .unwrap_err()
                .to_string()
                .contains("not synchronization")
        );
        let history = History::default();
        let a = insert(&history, "a", "s", "capture", 1000, Some(100));
        let b = insert(&history, "b", "s", "capture", 1000, Some(200));
        assert_eq!(history.convert(&a, &b, 150).unwrap(), (150, 12_000_340));
    }
    #[test]
    fn held_anchor_outlives_transport_and_unknown_new_session_stays_unavailable() {
        let history = History::default();
        let a = insert(&history, "peer", "s", "capture", 1000, None);
        // insert has already dropped the source runtime and capture handle.
        assert_eq!(history.describe(&a, 150)["timestamp_ns"], "1150");
        let next = crate::model::clock_ref(&auki_time::SessionClock::new("peer", "next-session", "capture").registry_entry());
        assert_eq!(history.describe(&next, 150)["status"], "unavailable");
        assert_eq!(history.describe(&a, 200)["timestamp_ns"], "1200");
    }

    #[test]
    fn reconnect_cannot_rewrite_an_existing_session_anchor() {
        for change_time in [true, false] {
            let history = History::default();
            let clock = insert(&history, "peer", "s", "capture", 1000, None);
            let (contract, product) = {
                let entries = history.0.lock().unwrap();
                let entry = entries.values().next().unwrap().lock().unwrap();
                (entry.contract.clone(), entry.anchor.as_ref().unwrap().clone())
            };
            // A fresh worker registers the same identity after rediscovery.
            let entry = history.register(contract, &product.manifest, &product.producer).unwrap();
            let mut observation = product.buffer().snapshot(0, 0)[0].payload.clone();
            entry.lock().unwrap().accept_anchor(&product, &observation).unwrap();
            if change_time { observation.timestamp_ns += 1; }
            else { observation.payload = Arc::new(Sample {offset_ns:1001,uncertainty_ns:20}); }
            assert!(entry.lock().unwrap().accept_anchor(&product, &observation).is_err());
            assert_eq!(history.describe(&clock, 150)["status"], "unavailable", "conflicting replay quarantines this identity");
            let next = insert(&history, "peer", "next-session", "capture", 2000, None);
            assert_eq!(history.describe(&next, 150)["timestamp_ns"], "2150");
        }
    }

    #[test]
    fn changed_contract_or_duplicate_product_quarantines_the_clock_even_when_offsets_agree() {
        for change_contract in [true, false] {
            let history = History::default();
            let clock = insert(&history, "peer", "s", "capture", 1000, None);
            let (mut contract, retained) = {
                let entries = history.0.lock().unwrap();
                let entry = entries.values().next().unwrap().lock().unwrap();
                (entry.contract.clone(), entry.anchor.as_ref().unwrap().clone())
            };
            let mut product = retained.manifest.clone();
            let mut output = retained.producer.clone();
            if change_contract {
                contract.utc_accuracy_ns = Some(1);
                output.output_id = contract.output_id();
                product.producer = output.reference();
            } else { product.product_id = "another-product-for-same-clock".into(); }
            assert!(history.register(contract, &product, &output).is_err());
            assert_eq!(history.describe(&clock, 150)["status"], "unavailable");
        }
    }

    #[test]
    fn transport_retention_cannot_overwrite_the_validated_anchor_or_its_provenance() {
        let history = History::default();
        insert(&history, "peer", "s", "capture", 1000, None);
        let entry = history.0.lock().unwrap().values().next().unwrap().clone();
        let retained = entry.lock().unwrap().anchor.as_ref().unwrap().clone();
        let transport = RetainedProduct::imported_buffer(retained.manifest.clone(), retained.manifest_hash.clone(),
            retained.producer.clone(), BufferLimits {max_entries:Some(1),max_bytes:Some(16384),target_duration:None},
            |_: &Sample| 16).unwrap();
        let original = retained.buffer().snapshot(0, 0)[0].payload.clone();
        let mut invalid = original.clone();
        invalid.sequence = 1;
        invalid.timestamp_ns += 1;
        invalid.payload = Arc::new(Sample {offset_ns:9999,uncertainty_ns:20});
        transport.buffer().append_shared(Arc::new(Envelope::new(invalid.sequence, invalid.timestamp_ns, invalid.clone()))).unwrap();
        assert!(entry.lock().unwrap().accept_anchor(&transport, &invalid).is_err());
        let still = retained.buffer().snapshot(0, 0)[0].payload.clone();
        assert_eq!(retained.reference(), transport.reference());
        assert_eq!(still.output, original.output);
        assert_eq!(still.sequence, original.sequence);
        assert_eq!(still.timestamp_ns, original.timestamp_ns);
        assert_eq!(still.payload, original.payload);
    }

    #[test]
    fn full_ledger_refuses_new_identities_without_eviction() {
        let history = History::default();
        let clock = insert(&history, "peer", "s", "capture", 1000, None);
        let (mut contract, retained) = {
            let mut entries = history.0.lock().unwrap();
            let entry = entries.values().next().unwrap().clone();
            for index in 1..MAX_CLOCK_IDENTITIES { entries.insert(format!("test-filler-{index}"), entry.clone()); }
            let e = entry.lock().unwrap();
            (e.contract.clone(), e.anchor.as_ref().unwrap().clone())
        };
        contract.from_clock.clock_id = "new-capture".into();
        let mut output = retained.producer.clone();
        output.clock = crate::model::clock_ref(&contract.from_clock);
        output.output_id = contract.output_id();
        let mut product = retained.manifest.clone();
        product.producer = output.reference();
        let error = history.register(contract, &product, &output).err().unwrap();
        assert!(error.to_string().contains("identity limit"));
        assert_eq!(history.0.lock().unwrap().len(), MAX_CLOCK_IDENTITIES);
        assert_eq!(history.describe(&clock, 150)["timestamp_ns"], "1150");
    }

    #[test]
    fn lookup_requires_the_streams_full_clock_identity_not_its_peer() {
        let history = History::default();
        let camera = insert(&history, "peer", "s", "camera", 1000, None);
        let depth = insert(&history, "peer", "s", "depth", 2000, None);
        assert_eq!(history.describe(&camera, 150)["timestamp_ns"], "1150");
        assert_eq!(history.describe(&depth, 150)["timestamp_ns"], "2150");
        for wrong in [
            ClockReference {
                peer_id: "other".into(),
                ..camera.clone()
            },
            ClockReference {
                id: depth.id.clone(),
                ..camera.clone()
            },
            ClockReference {
                hash: depth.hash.clone(),
                ..camera.clone()
            },
            crate::model::clock_ref(
                &auki_time::SessionClock::new("peer", "next-session", "camera").registry_entry(),
            ),
        ] {
            assert_eq!(history.describe(&wrong, 150)["status"], "unavailable");
        }
    }
}
