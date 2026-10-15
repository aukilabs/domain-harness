//! Home-only reader for PR40 camera-space depth and declared transform chains.
use crate::compact_pose::{FrameRef, Update};
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{ComponentProtocolClient, ObservationRequest, ObservationSelection,
    ObservationStart, RemoteObservationEvent};
use auki_components::*;
use std::{collections::BTreeMap, sync::{Arc, Mutex}, time::Duration};
use tokio_util::sync::CancellationToken;
use crate::depth_points::{PackedPoints, PACKED_POINTS_SCHEMA};
pub use crate::mobile_declared::{Points, PointsContract, POINTS_SCHEMA as SCHEMA, POINTS_METADATA as METADATA};
pub const MAX_GAP_NS: u64 = 500_000_000;

impl PointsContract {
    pub fn validate_output(&self, o: &OutputManifest) -> Result<()> {
        self.validate_typed_output(o, SCHEMA)
    }

    pub fn validate_packed_output(&self, o: &OutputManifest) -> Result<()> {
        self.validate_typed_output(o, PACKED_POINTS_SCHEMA)
    }

    fn validate_typed_output(&self, o: &OutputManifest, schema: &str) -> Result<()> {
        o.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            !self.session_id.is_empty() && self.session_id.len() <= 256,
            "Invalid depth session"
        );
        ensure!(
            self.frame.peer_id == o.peer_id && self.clock.peer_id == o.peer_id,
            "Depth registry owner mismatch"
        );
        ensure!(
            o.output_id == format!("points-{}", manifest_hash(self)) && o.clock == self.clock,
            "Depth descriptor mismatch"
        );
        ensure!(
            matches!(&o.payload, PayloadContract::Structured(p) if p.datatype==schema && p.schema==schema),
            "Wrong depth type"
        );
        ensure!(
            o.spatial_frame
                .as_ref()
                .is_some_and(|f| FrameRef::from(&f.definition) == self.frame),
            "Depth frame mismatch"
        );
        Ok(())
    }
}
pub(crate) struct Localized {
    pub target: String,
    pub binding: auki_voxel_map::SensorBinding,
    pub sequence: u64,
    pub timestamp: u64,
    pub points: Points,
    pub pose: auki_odometry::Pose,
}

/// Only exact samples or consecutive valid brackets in one validated Output.
/// The quaternion interpolation is shortest-arc SLERP, as in SDK odometry.
pub fn sampled_at(samples: &[(u64, u64, Update)], t: u64) -> Result<auki_odometry::Pose> {
    ensure!(
        samples
            .windows(2)
            .all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1),
        "Non-monotonic pose history"
    );
    let a = samples
        .iter()
        .rev()
        .find(|s| s.1 <= t)
        .ok_or_else(|| anyhow!("Pose before capture unavailable"))?;
    let b = samples
        .iter()
        .find(|s| s.1 >= t)
        .ok_or_else(|| anyhow!("Pose after capture unavailable; no extrapolation"))?;
    a.2.validate()?;
    b.2.validate()?;
    let (auki_odometry::Tracking::Valid { pose: pa }, auki_odometry::Tracking::Valid { pose: pb }) =
        (&a.2.tracking, &b.2.tracking)
    else {
        anyhow::bail!("Tracking lost at capture");
    };
    if a.0 == b.0 {
        return Ok(pa.clone());
    }
    ensure!(
        b.0 == a.0 + 1 && b.1 - a.1 <= MAX_GAP_NS,
        "Pose gap exceeds interpolation limit"
    );
    let fraction = (t - a.1) as f64 / (b.1 - a.1) as f64;
    let qa = pa.rotation_xyzw;
    let mut qb = pb.rotation_xyzw;
    let mut dot: f64 = qa.iter().zip(qb).map(|(a, b)| a * b).sum();
    if dot < 0. {
        qb = qb.map(|v| -v);
        dot = -dot;
    }
    let (wa, wb) = if dot > 0.9995 {
        (1. - fraction, fraction)
    } else {
        let angle = dot.clamp(-1., 1.).acos();
        (
            ((1. - fraction) * angle).sin() / angle.sin(),
            (fraction * angle).sin() / angle.sin(),
        )
    };
    let mut q: [f64; 4] = std::array::from_fn(|i| wa * qa[i] + wb * qb[i]);
    let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    q = q.map(|v| v / norm);
    let result = auki_odometry::Pose {
        translation: std::array::from_fn(|i| {
            pa.translation[i] * (1. - fraction) + pb.translation[i] * fraction
        }),
        rotation_xyzw: q,
    };
    result.validate()?;
    Ok(result)
}

async fn read<T: ContractType + serde::de::DeserializeOwned>(
    client: &ComponentProtocolClient,
    sensor: &crate::sensor_preview::Sensor,
    selection: ObservationSelection,
) -> Result<auki_component_protocol::RemoteObservations<T>> {
    let request = ObservationRequest {
        product: sensor.reference.clone(),
        selection,
    };
    Ok(if let Some(route) = &sensor.route {
        client
            .observations_exact::<T>(sensor.peer, route.clone(), request)
            .await?
    } else {
        client.observations::<T>(sensor.peer, request).await?
    })
}

async fn capture(
    client: &ComponentProtocolClient,
    held_cache: &crate::frame_alignment::HeldTransforms,
    owner: &str,
    row: &serde_json::Value,
    home: &str,
    scenes: &crate::local_scenes::LocalScenes,
    done: &mut BTreeMap<String, u64>,
) -> Result<()> {
    let catalog: CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
    for p in &catalog.products {
        let Some(m) = p.metadata.as_ref().filter(|m| m.schema == METADATA) else {
            continue;
        };
        ensure!(p.manifest.peer_id == owner, "Foreign depth Product");
        let c: PointsContract = serde_json::from_value(m.value.clone())?;
        let output = catalog
            .components
            .iter()
            .flat_map(|c| c.current_outputs.values())
            .find(|o| o.manifest.reference() == p.manifest.producer)
            .ok_or_else(|| anyhow!("Depth Output unavailable"))?;
        if matches!(&output.manifest.payload, PayloadContract::Structured(p)
            if p.datatype == PACKED_POINTS_SCHEMA && p.schema == PACKED_POINTS_SCHEMA) { continue; }
        c.validate_output(&output.manifest)?;
        let sensor = crate::sensor_preview::Sensor {
            datatype: SCHEMA.into(),
            id: p.manifest.product_id.clone(),
            reference: p.manifest.reference(),
            route: row["catalog_route"].as_str().and_then(|s| s.parse().ok()),
            peer: owner.parse()?,
            metadata: Some(m.clone()),
            ar_supported: false,
        };
        let r = read::<Points>(client, &sensor, ObservationSelection::LatestExisting).await?;
        ensure!(
            r.product.reference() == sensor.reference
                && r.product.producer == r.producer.reference()
                && r.gap.is_none()
                && r.end.is_none(),
            "Depth Product unavailable or changed"
        );
        c.validate_output(&r.producer)?;
        let o = r
            .observations
            .last()
            .ok_or_else(|| anyhow!("Depth Product empty"))?;
        ensure!(
            o.output == r.producer.reference(),
            "Depth provenance mismatch"
        );
        o.payload.validate()?;
        let key = serde_json::to_string(&sensor.reference)?;
        if done.get(&key).is_some_and(|seq| *seq >= o.sequence) {
            continue;
        }
        integrate(client, held_cache, &sensor, row, home, scenes, &c, &r.producer,
            o.sequence, o.timestamp_ns, (*o.payload).clone()).await?;
        done.insert(key, o.sequence);
    }
    Ok(())
}

/// Both wire profiles use exactly the same registered capture-time transform chain.
async fn integrate(
    client: &ComponentProtocolClient,
    held_cache: &crate::frame_alignment::HeldTransforms,
    sensor: &crate::sensor_preview::Sensor,
    row: &serde_json::Value,
    home: &str,
    scenes: &crate::local_scenes::LocalScenes,
    c: &PointsContract,
    producer: &OutputManifest,
    sequence: u64,
    timestamp: u64,
    points: Points,
) -> Result<()> {
    let editors = scenes.note_editors.lock().map_err(|_|anyhow!("Scenes unavailable"))?
        .values().cloned().collect::<Vec<_>>();
    let targets = editors.iter().map(|editor| {
        let reference = editor.reference()?;
        crate::conventions::registry(home, &reference.scenegraph.map.frame.id)
            .map(|f| FrameRef::from(&f))
    }).collect::<Result<Vec<_>>>()?;
    let (target, pose) = crate::transform_chain::resolve(client, sensor, row,
        &c.frame, &c.clock, &c.session_id, timestamp, &targets, Some(held_cache)).await?;
    let source_frame = producer.spatial_frame.as_ref()
        .ok_or_else(||anyhow!("Depth frame missing"))?.definition.clone();
    let editor = editors.into_iter().find(|editor| editor.reference().is_ok_and(|r|
        r.scenegraph.map.frame.id == target.id)).ok_or_else(||anyhow!("Home scene unavailable"))?;
    editor.integrate_declared(Localized {
        target: target.id,
        binding: auki_voxel_map::SensorBinding {
            sensor_product: sensor.reference.clone(),
            sensor_frame: source_frame,
            observation_clock: c.clock.clone(),
        },
        sequence,
        timestamp,
        points,
        pose,
    })?;
    Ok(())
}

const MAX_PACKED_STREAMS: usize = 64;

struct PackedWorker {
    fingerprint: String,
    sensor: crate::sensor_preview::Sensor,
    // The SDK owns retention; one latest observation, never a second cloud queue.
    product: Arc<Mutex<Option<RetainedProduct<PackedPoints>>>>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

fn packed_sensors(rows: &crate::device_catalogs::Catalogs)
    -> BTreeMap<String, (String, crate::sensor_preview::Sensor)> {
    let mut desired = BTreeMap::new();
    for (owner, row) in rows {
        let Ok(catalog) = serde_json::from_value::<CatalogSnapshot>(row["catalog"].clone()) else { continue; };
        for p in &catalog.products {
            if p.manifest.peer_id != *owner { continue; }
            let Some(output) = catalog.components.iter().flat_map(|c| c.current_outputs.values())
                .find(|o| o.manifest.reference() == p.manifest.producer) else { continue; };
            // An idle demand-driven Product has no first observation or metadata yet.
            // Its exact typed Output is enough to subscribe; the packet supplies the
            // mandatory frame/clock/session contract before any sample is integrated.
            if output.manifest.validate().is_err() || output.manifest.spatial_frame.is_none()
                || !matches!(&output.manifest.payload, PayloadContract::Structured(s)
                    if s.datatype == PACKED_POINTS_SCHEMA && s.schema == PACKED_POINTS_SCHEMA) { continue; }
            let Ok(peer) = owner.parse() else { continue; };
            let Some(route) = row["catalog_route"].as_str().and_then(|s| s.parse().ok()) else { continue; };
            let reference = p.manifest.reference();
            let key = serde_json::to_string(&reference).unwrap();
            let fingerprint = serde_json::to_string(&(&reference, &row["catalog_route"])).unwrap();
            desired.insert(key, (fingerprint, crate::sensor_preview::Sensor {
                peer, route: Some(route),
                id: p.manifest.product_id.clone(), reference, datatype: PACKED_POINTS_SCHEMA.into(),
                metadata: p.metadata.clone(), ar_supported: false,
            }));
        }
    }
    desired
}

async fn follow_packed(client: ComponentProtocolClient, sensor: crate::sensor_preview::Sensor,
    product: Arc<Mutex<Option<RetainedProduct<PackedPoints>>>>, stop: CancellationToken) {
    let mut backoff = 1;
    let mut last_error = String::new();
    loop {
        *product.lock().unwrap() = None;
        let run = async {
            let limits = crate::observation_history::depth();
            let connect = async {
                let size = |points: &PackedPoints| points.as_bytes().len();
                client.subscribe_product_exact_with_decoder(sensor.peer, sensor.route.clone().unwrap(), sensor.reference.clone(),
                    ObservationStart::NewOnly, limits, size, PackedPoints::decoder()?).await
            };
            let mut sub = tokio::time::timeout(Duration::from_secs(5), connect).await??;
            ensure!(sub.product().reference() == sensor.reference
                && sub.product().manifest.producer == sub.product().producer.reference(), "Depth Product changed");
            let producer = sub.product().producer.clone();
            *product.lock().unwrap() = Some(sub.product().clone());
            let result: Result<()> = async {
                loop {
                    match sub.next().await? {
                        Some(RemoteObservationEvent::Observation(o)) => {
                            ensure!(o.output == producer.reference(), "Depth provenance mismatch");
                            // The binary decoder constructs only immutable, validated packets.
                            o.payload.contract().validate_packed_output(&producer)?;
                            backoff = 1;
                            last_error.clear();
                        }
                        Some(RemoteObservationEvent::Gap(_)) => {}, // Latest-cloud mapping skips superseded samples.
                        Some(RemoteObservationEvent::Closed(_)) | None => return Ok(()),
                    }
                }
            }.await;
            *product.lock().unwrap() = None;
            let _ = tokio::time::timeout(Duration::from_secs(1), sub.close()).await;
            result
        };
        let result = tokio::select! {_ = stop.cancelled() => break, result = run => result};
        match result {
            Ok(()) => break, // A terminal Product needs a newly discovered identity.
            Err(error) => {
                *product.lock().unwrap() = None;
                let detail = error.to_string();
                if detail != last_error {
                    eprintln!("Declared depth stream {} / {}: {detail}", sensor.peer, sensor.id);
                    last_error = detail;
                }
            }
        }
        tokio::select! {_ = stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}}
        backoff = (backoff * 2).min(8);
    }
    *product.lock().unwrap() = None;
}

async fn capture_packed(client: &ComponentProtocolClient,
    held_cache: &crate::frame_alignment::HeldTransforms, worker: &PackedWorker,
    row: &serde_json::Value, home: &str, scenes: &crate::local_scenes::LocalScenes,
    done: &mut BTreeMap<String, u64>) -> Result<()> {
    let Some(product) = worker.product.lock().map_err(|_|anyhow!("Depth stream unavailable"))?.clone() else { return Ok(()); };
    let Some(o) = product.latest_existing()? else { return Ok(()); };
    let key = serde_json::to_string(&worker.sensor.reference)?;
    if done.get(&key).is_some_and(|seq| *seq >= o.sequence) { return Ok(()); }
    ensure!(o.output == product.producer.reference(), "Depth provenance mismatch");
    let c = o.payload.contract().clone();
    c.validate_packed_output(&product.producer)?;
    integrate(client, held_cache, &worker.sensor, row, home, scenes, &c, &product.producer,
        o.sequence, o.timestamp_ns, (*o.payload).clone().into_points()).await?;
    done.insert(key, o.sequence);
    Ok(())
}

pub async fn serve(
    peer: &auki_sdk::AukiPeer,
    catalogs: tokio::sync::watch::Receiver<crate::device_catalogs::Catalogs>,
    scenes: &crate::local_scenes::LocalScenes,
    held_cache: &crate::frame_alignment::HeldTransforms,
    stop: &tokio_util::sync::CancellationToken,
) {
    let client = ComponentProtocolClient::new(peer.protocols());
    let home = peer.peer_id().to_string();
    let mut done = BTreeMap::new();
    let mut errors = BTreeMap::<String, String>::new();
    let lifetime = stop.child_token();
    let _cancel_on_drop = lifetime.clone().drop_guard();
    let mut workers = BTreeMap::<String, PackedWorker>::new();
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {_ = stop.cancelled()=>break,_ = tick.tick()=>{}}
        let rows = catalogs.borrow().clone();
        let desired = packed_sensors(&rows);
        let removed = workers.iter().filter(|(key, worker)|
            desired.get(*key).is_none_or(|(fingerprint, _)| fingerprint != &worker.fingerprint))
            .map(|(key, _)|key.clone()).collect::<Vec<_>>();
        for key in removed {
            let worker = workers.remove(&key).unwrap();
            worker.stop.cancel();
            let _ = worker.task.await;
            done.remove(&key);
        }
        for (key, (fingerprint, sensor)) in desired {
            if workers.contains_key(&key) || workers.len() >= MAX_PACKED_STREAMS { continue; }
            let product = Arc::new(Mutex::new(None));
            let cancel = lifetime.child_token();
            let task = tokio::spawn(follow_packed(client.clone(), sensor.clone(), product.clone(), cancel.clone()));
            workers.insert(key, PackedWorker {fingerprint, sensor, product, stop:cancel, task});
        }
        errors.retain(|key, _| rows.contains_key(key) || workers.contains_key(key));
        for (owner, row) in &rows {
            let result = tokio::select! {_ = stop.cancelled()=>break,r=tokio::time::timeout(Duration::from_secs(2),capture(&client,held_cache,owner,row,&home,scenes,&mut done))=>r.unwrap_or_else(|_|Err(anyhow!("Declared depth resolution timed out")))};
            match result {
                Ok(()) => {
                    errors.remove(owner);
                }
                Err(e) => {
                    let detail = e.to_string();
                    if errors.get(owner) != Some(&detail) {
                        eprintln!("Declared depth {owner}: {detail}");
                        errors.insert(owner.clone(), detail);
                    }
                }
            }
        }
        for (key, worker) in &workers {
            let Some(row) = rows.get(&worker.sensor.peer.to_string()) else { continue; };
            let result = tokio::select! {_ = stop.cancelled()=>break,
                r=tokio::time::timeout(Duration::from_secs(2), capture_packed(&client, held_cache, worker, row, &home, scenes, &mut done))
                    => r.unwrap_or_else(|_|Err(anyhow!("Declared depth resolution timed out")))};
            match result {
                Ok(()) => { errors.remove(key); }
                Err(error) => {
                    let detail = error.to_string();
                    if errors.get(key) != Some(&detail) {
                        eprintln!("Declared depth {} / {}: {detail}", worker.sensor.peer, worker.sensor.id);
                        errors.insert(key.clone(), detail);
                    }
                }
            }
        }
        // Product identities change across sessions. Bound bookkeeping, not capture data.
        if done.len() > 1024 {
            done.clear();
        }
    }
    lifetime.cancel();
    for (_, worker) in workers {
        worker.stop.cancel();
        let _ = worker.task.await;
    }
}
