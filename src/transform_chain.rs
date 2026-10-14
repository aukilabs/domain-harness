//! Capture-time transform resolution shared by image and point-cloud consumers.
use crate::compact_pose::{Contract, FrameRef, Lifetime, Update};
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{ComponentProtocolClient, ObservationRequest, ObservationSelection};
use auki_components::*;
use auki_odometry::Pose;
use serde_json::Value;
const MAX_EDGES: usize = 64;
const MAX_DEPTH: usize = 8;
const GAP_NS: u64 = 250_000_000;

#[derive(Clone)]
struct Edge {
    product: ProductReference,
    output: OutputManifest,
    contract: Contract,
    advertised_sequence: u64,
}

fn edges(catalog: &CatalogSnapshot, frame: &FrameRef, clock: &ClockReference, session: &str) -> Result<Vec<Edge>> {
    let mut result = Vec::new();
    for p in &catalog.products {
        let Some(m) = &p.metadata else { continue };
        if m.schema != crate::compact_pose::METADATA {
            continue;
        }
        let c: Contract = serde_json::from_value(m.value.clone())?;
        // No implicit cross-clock conversion or cross-session interpolation.
        if c.clock != *clock || c.session_id != session {
            continue;
        }
        let output = catalog
            .components
            .iter()
            .flat_map(|c| c.current_outputs.values())
            .find(|o| o.manifest.reference() == p.manifest.producer)
            .ok_or_else(|| anyhow!("Transform producer unavailable"))?;
        ensure!(
            p.manifest.hash() == p.manifest_hash && p.manifest.peer_id == frame.peer_id,
            "Transform Product identity mismatch"
        );
        c.validate_output(&output.manifest)?;
        result.push(Edge {
            product: p.manifest.reference(),
            output: output.manifest.clone(),
            contract: c,
            advertised_sequence: m.source_sequence,
        });
    }
    ensure!(
        result.len() <= MAX_EDGES,
        "AR transform graph exceeds limit"
    );
    Ok(result)
}

// Deliberately directed: publishers declare the direction they vouch for. No
// shortest-path preference silently hides a competing calibration.
fn unique_path(
    edges: &[Edge],
    start: &FrameRef,
    targets: &[FrameRef],
) -> Result<(FrameRef, Vec<usize>)> {
    fn visit(
        edges: &[Edge],
        at: &FrameRef,
        targets: &[FrameRef],
        seen: &mut Vec<FrameRef>,
        path: &mut Vec<usize>,
        found: &mut Vec<(FrameRef, Vec<usize>)>,
        budget: &mut usize,
    ) {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        if found.len() > 1 {
            return;
        }
        if targets.contains(at) {
            found.push((at.clone(), path.clone()));
            return;
        }
        if path.len() == MAX_DEPTH {
            return;
        }
        for (i, e) in edges.iter().enumerate() {
            if e.contract.from_frame != *at || seen.contains(&e.contract.to_frame) {
                continue;
            }
            seen.push(e.contract.to_frame.clone());
            path.push(i);
            visit(
                edges,
                &e.contract.to_frame,
                targets,
                seen,
                path,
                found,
                budget,
            );
            path.pop();
            seen.pop();
        }
    }
    let mut found = Vec::new();
    let mut budget = 1024;
    visit(
        edges,
        start,
        targets,
        &mut vec![start.clone()],
        &mut Vec::new(),
        &mut found,
        &mut budget,
    );
    ensure!(budget > 0, "AR transform graph search exceeds limit");
    ensure!(
        found.len() == 1,
        "No unambiguous registry transform chain into Home"
    );
    Ok(found.remove(0))
}

async fn at(
    client: &ComponentProtocolClient,
    sensor: &crate::sensor_preview::Sensor,
    edge: &Edge,
    timestamp: u64,
) -> Result<Pose> {
    let c = &edge.contract;
    let (start_ns, end_ns) = match c.lifetime {
        Lifetime::Held => (0, timestamp),
        Lifetime::Sampled => (
            timestamp.saturating_sub(GAP_NS),
            timestamp.saturating_add(GAP_NS),
        ),
    };
    let request = ObservationRequest {
        product: edge.product.clone(),
        selection: ObservationSelection::TimeRange {
            request: TimeRangeRequest {
                clock: c.clock.clone(),
                start_ns,
                end_ns,
            },
        },
    };
    let r = if let Some(route) = &sensor.route {
        client
            .observations_exact::<Update>(sensor.peer, route.clone(), request)
            .await?
    } else {
        client.observations::<Update>(sensor.peer, request).await?
    };
    ensure!(
        r.product.reference() == edge.product
            && r.producer.reference() == edge.output.reference()
            && r.product.producer == r.producer.reference()
            && r.gap.is_none()
            && r.end.is_none(),
        "AR transform Product changed, ended or has a gap"
    );
    c.validate_output(&r.producer)?;
    let mut samples = Vec::new();
    for o in r.observations {
        ensure!(
            o.output == r.producer.reference()
                && o.timestamp_ns >= start_ns
                && o.timestamp_ns <= end_ns,
            "AR observation provenance/time mismatch"
        );
        if let Some((seq, time, _)) = samples.last() {
            ensure!(
                o.sequence == seq + 1 && o.timestamp_ns > *time,
                "AR transform history gap/order mismatch"
            );
        }
        o.payload.validate()?;
        samples.push((o.sequence, o.timestamp_ns, (*o.payload).clone()));
    }
    sample_at(&samples, c.lifetime, timestamp)
}

fn sample_at(samples: &[(u64, u64, Update)], lifetime: Lifetime, timestamp: u64) -> Result<Pose> {
    match lifetime {
        Lifetime::Held => crate::frame_alignment::held_at(
            &samples
                .iter()
                .map(|(_, t, u)| (*t, u.clone()))
                .collect::<Vec<_>>(),
            timestamp,
        )
        .map(|v| v.0),
        Lifetime::Sampled => {
            // Only missing bounds are retryable. Loss and excessive gaps are not.
            if let Some(before) = samples.iter().rev().find(|s| s.1 <= timestamp) {
                before.2.validate()?;
                ensure!(!matches!(before.2.tracking, auki_odometry::Tracking::Lost),
                    "Tracking lost at capture");
            }
            ensure!(
                samples.iter().any(|s| s.1 <= timestamp)
                    && samples.iter().any(|s| s.1 >= timestamp),
                "Waiting for capture-matched camera transform"
            );
            let before = samples.iter().rev().find(|s| s.1 <= timestamp).unwrap();
            let after = samples.iter().find(|s| s.1 >= timestamp).unwrap();
            ensure!(after.1 - before.1 <= GAP_NS, "AR pose gap exceeds 250 ms");
            crate::declared_depth::sampled_at(samples, timestamp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid(x: f64) -> Update {
        Update {
            tracking: auki_odometry::Tracking::Valid {
                pose: Pose {
                    translation: [x, 0., 0.],
                    rotation_xyzw: [0., 0., 0., 1.],
                },
            },
        }
    }
    #[test]
    fn sampled_capture_never_extrapolates_or_crosses_loss_or_gaps() {
        let samples = vec![(0, 100, valid(0.)), (1, 200, valid(2.))];
        assert_eq!(
            sample_at(&samples, Lifetime::Sampled, 150)
                .unwrap()
                .translation,
            [1., 0., 0.]
        );
        assert!(sample_at(&samples, Lifetime::Sampled, 99).is_err());
        assert!(sample_at(&samples, Lifetime::Sampled, 201).is_err());
        let mut lost = samples.clone();
        lost[1].2 = Update {
            tracking: auki_odometry::Tracking::Lost,
        };
        assert!(
            sample_at(&lost, Lifetime::Sampled, 150)
                .unwrap_err()
                .to_string()
                .contains("lost")
        );
        assert!(sample_at(&lost, Lifetime::Sampled, 201).unwrap_err().to_string().contains("lost"));
        let mut gap = samples.clone();
        gap[1].0 = 3;
        assert!(sample_at(&gap, Lifetime::Sampled, 150).is_err());
        let mut slow = samples;
        slow[1].1 = GAP_NS + 101;
        assert!(sample_at(&slow, Lifetime::Sampled, 150).is_err());
    }
    #[test]
    fn held_state_uses_capture_time_and_never_searches_past_revocation() {
        let samples = vec![
            (0, 10, valid(1.)),
            (
                1,
                20,
                Update {
                    tracking: auki_odometry::Tracking::Lost,
                },
            ),
            (2, 30, valid(2.)),
        ];
        assert_eq!(
            sample_at(&samples, Lifetime::Held, 15).unwrap().translation[0],
            1.
        );
        assert!(sample_at(&samples, Lifetime::Held, 25).is_err());
        assert_eq!(
            sample_at(&samples, Lifetime::Held, u64::MAX)
                .unwrap()
                .translation[0],
            2.
        );
        assert!(sample_at(&samples, Lifetime::Held, 9).is_err());
    }
}


/// Resolve one unambiguous declared chain at the observation's own capture time.
/// Every edge must share the exact source clock/session; never extrapolate.
pub async fn resolve(
    client: &ComponentProtocolClient, sensor: &crate::sensor_preview::Sensor,
    row: &Value, frame: &FrameRef, clock: &ClockReference, session: &str,
    timestamp: u64, targets: &[FrameRef], held: Option<&crate::frame_alignment::HeldTransforms>,
) -> Result<(FrameRef, Pose)> {
    ensure!(row["catalog_status"] == "ready", "Transform Catalog unavailable");
    ensure!(frame.peer_id == sensor.reference.peer_id && clock.peer_id == frame.peer_id, "Transform source authority mismatch");
    let catalog: CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
    let edges = edges(&catalog, frame, clock, session)?;
    let (target, path) = unique_path(&edges, frame, targets)?;
    let mut pose = crate::local_mapper::identity();
    for index in path {
        let edge = &edges[index];
        crate::product_registry::resolved_contract(&row["registries"], &edge.contract)?;
        let transform = if let (Lifetime::Held, Some(cache)) = (edge.contract.lifetime, held) {
            cache.at(&edge.product, &edge.contract, edge.advertised_sequence, timestamp)?.0
        } else { at(client, sensor, edge, timestamp).await? };
        pose = crate::local_mapper::compose(&transform, &pose)?;
    }
    Ok((target, pose))
}
