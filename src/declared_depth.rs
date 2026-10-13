//! Home-only reader for PR40 camera-space depth and declared transform chains.
use crate::compact_pose::{Contract, FrameRef, Lifetime, Update};
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{ComponentProtocolClient, ObservationRequest, ObservationSelection};
use auki_components::*;
use std::{collections::BTreeMap, time::Duration};
pub use crate::mobile_declared::{Points, PointsContract, POINTS_SCHEMA as SCHEMA, POINTS_METADATA as METADATA};
pub const MAX_GAP_NS: u64 = 500_000_000;

impl PointsContract {
    pub fn validate_output(&self, o: &OutputManifest) -> Result<()> {
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
            matches!(&o.payload, PayloadContract::Structured(p) if p.datatype==SCHEMA && p.schema==SCHEMA),
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
        c.validate_output(&output.manifest)?;
        let sensor = crate::sensor_preview::Sensor {
            datatype: SCHEMA.into(),
            id: p.manifest.product_id.clone(),
            reference: p.manifest.reference(),
            route: row["catalog_route"].as_str().and_then(|s| s.parse().ok()),
            peer: owner.parse()?,
            metadata: Some(m.clone()),
            pose: None,
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
        let candidates = catalog
            .products
            .iter()
            .filter_map(|p| {
                let m = p
                    .metadata
                    .as_ref()
                    .filter(|m| m.schema == crate::compact_pose::METADATA)?;
                let pc: Contract = serde_json::from_value(m.value.clone()).ok()?;
                if p.manifest.peer_id != owner
                    || pc.lifetime != Lifetime::Sampled
                    || pc.from_frame != c.frame
                    || pc.clock != c.clock
                    || pc.session_id != c.session_id
                {
                    return None;
                }
                let out = catalog
                    .components
                    .iter()
                    .flat_map(|c| c.current_outputs.values())
                    .find(|out| out.manifest.reference() == p.manifest.producer)?;
                pc.validate_output(&out.manifest).ok()?;
                Some((p, pc))
            })
            .collect::<Vec<_>>();
        ensure!(
            candidates.len() == 1,
            "No unambiguous sampled pose for depth frame"
        );
        let (pp, pc) = &candidates[0];
        let (resolved, _) = crate::product_registry::resolved_contract(&row["registries"], pc)?;
        let ps = crate::sensor_preview::Sensor {
            reference: pp.manifest.reference(),
            ..sensor.clone()
        };
        let t = o.timestamp_ns;
        let poses = read::<Update>(
            client,
            &ps,
            ObservationSelection::TimeRange {
                request: TimeRangeRequest {
                    clock: c.clock.clone(),
                    start_ns: t.saturating_sub(MAX_GAP_NS),
                    end_ns: t.saturating_add(MAX_GAP_NS),
                },
            },
        )
        .await?;
        ensure!(
            poses.product.reference() == ps.reference
                && poses.product.producer == poses.producer.reference()
                && poses.gap.is_none()
                && poses.end.is_none(),
            "Pose history unavailable"
        );
        pc.validate_output(&poses.producer)?;
        let samples = poses
            .observations
            .iter()
            .map(|s| {
                ensure!(
                    s.output == poses.producer.reference(),
                    "Pose provenance mismatch"
                );
                Ok((s.sequence, s.timestamp_ns, (*s.payload).clone()))
            })
            .collect::<Result<Vec<_>>>()?;
        let native = sampled_at(&samples, t)?;
        let editors = scenes
            .note_editors
            .lock()
            .map_err(|_| anyhow!("Scenes unavailable"))?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut matched = Vec::new();
        for editor in editors {
            let reference = editor.reference()?;
            let target = &reference.scenegraph.map.frame.id;
            if !catalog.products.iter().any(|p| {
                p.metadata.as_ref().is_some_and(|m| {
                    m.schema == crate::compact_pose::METADATA
                        && serde_json::from_value::<Contract>(m.value.clone()).is_ok_and(|held| {
                            crate::frame_alignment::validate_chain(&held, pc, owner, home, target)
                                .is_ok()
                        })
                })
            }) {
                continue;
            }
            let (held, _) = crate::frame_alignment::resolve(
                held_cache,
                &ps,
                row,
                pc,
                home,
                &serde_json::json!({"frame":target}),
                t,
            )?;
            matched.push((
                editor,
                target.clone(),
                crate::local_mapper::compose(&held, &native)?,
            ));
        }
        ensure!(
            !matched.is_empty(),
            "No declared transform into a Home scene"
        );
        for (editor, target, pose) in matched {
            editor.integrate_declared(Localized {
                target,
                binding: auki_voxel_map::SensorBinding {
                    sensor_product: sensor.reference.clone(),
                    sensor_frame: resolved.from_frame.clone(),
                    observation_clock: c.clock.clone(),
                },
                sequence: o.sequence,
                timestamp: t,
                points: (*o.payload).clone(),
                pose,
            })?;
        }
        done.insert(key, o.sequence);
    }
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
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {_ = stop.cancelled()=>break,_ = tick.tick()=>{}}
        let rows = catalogs.borrow().clone();
        for (owner, row) in rows {
            let result = tokio::select! {_ = stop.cancelled()=>return,r=tokio::time::timeout(Duration::from_secs(2),capture(&client,held_cache,&owner,&row,&home,scenes,&mut done))=>r.unwrap_or_else(|_|Err(anyhow!("Declared depth resolution timed out")))};
            match result {
                Ok(()) => {
                    errors.remove(&owner);
                }
                Err(e) => {
                    let detail = e.to_string();
                    if errors.get(&owner) != Some(&detail) {
                        eprintln!("Declared depth {owner}: {detail}");
                        errors.insert(owner, detail);
                    }
                }
            }
        }
        // Product identities change across sessions. Bound bookkeeping, not capture data.
        if done.len() > 1024 {
            done.clear();
        }
    }
}
