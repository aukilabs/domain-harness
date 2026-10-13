//! Home consumes held frame relationships through the compact pose profile.
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

pub async fn resolve(
    client: &auki_component_protocol::ComponentProtocolClient,
    sensor: &crate::sensor_preview::Sensor,
    row: &serde_json::Value,
    pose: &Contract,
    home_peer: &str,
    view: &serde_json::Value,
    timestamp: u64,
) -> Result<(auki_odometry::Pose, u64)> {
    use auki_component_protocol::{ObservationRequest, ObservationSelection};
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
            Some((p.manifest.reference(), c))
        })
        .collect::<Vec<_>>();
    ensure!(
        candidates.len() == 1,
        "No unambiguous held transform to this Home frame"
    );
    let (reference, c) = &candidates[0];
    crate::product_registry::resolved_contract(&row["registries"], c)?;
    // Read the retained history through t. LatestExisting would incorrectly use
    // a calibration from the future for an older capture. Do not reuse cached
    // validity after a failed read: that could mask a revocation.
    let request = ObservationRequest {
        product: reference.clone(),
        selection: ObservationSelection::TimeRange {
            request: TimeRangeRequest {
                clock: c.clock.clone(),
                start_ns: 0,
                end_ns: timestamp,
            },
        },
    };
    let r = tokio::time::timeout(std::time::Duration::from_millis(750), async {
        if let Some(route) = &sensor.route {
            client
                .observations_exact::<Update>(sensor.peer, route.clone(), request)
                .await
        } else {
            client.observations::<Update>(sensor.peer, request).await
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("Scene transform fetch timed out"))??;
    ensure!(
        r.product.reference() == *reference
            && r.product.producer == r.producer.reference()
            && r.producer.peer_id == sensor.reference.peer_id
            && r.gap.is_none()
            && r.end.is_none(),
        "Scene transform Product changed or unavailable"
    );
    c.validate_output(&r.producer)?;
    let mut samples = Vec::new();
    let mut previous = None;
    for o in r.observations {
        ensure!(
            o.output == r.producer.reference() && o.timestamp_ns <= timestamp,
            "Transform observation provenance mismatch"
        );
        if let Some(sequence) = previous {
            ensure!(o.sequence == sequence + 1, "Transform history gap");
        }
        previous = Some(o.sequence);
        o.payload.validate()?;
        samples.push((o.timestamp_ns, (*o.payload).clone()));
    }
    held_at(&samples, timestamp)
}
