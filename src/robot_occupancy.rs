//! Replaceable robot collision envelopes, separate from accumulated environment evidence.
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ObservationRequest, ObservationSelection,
};
use auki_components::{ClockReference, ContractType, PayloadContract, ProductReference};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RobotOccupancy {
    pub session_id: String,
    pub model_sha256: String,
    pub voxel_size_m: f64,
    pub grid_origin_m: [f64; 3],
    pub cells: Vec<[i32; 3]>,
    pub covered_links: Vec<String>,
    pub missing_links: Vec<String>,
    pub approximate_links: Vec<String>,
    pub compute_ms: f64,
}
impl ContractType for RobotOccupancy {
    const DATATYPE: &'static str = "spatial.robot-occupancy";
}
impl RobotOccupancy {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.session_id.is_empty() && self.session_id.len() <= 128,
            "occupancy session missing"
        );
        ensure!(
            self.model_sha256.len() == 64
                && self.model_sha256.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid model digest"
        );
        ensure!(
            (0.02..=0.1).contains(&self.voxel_size_m) && self.grid_origin_m == [0.; 3],
            "invalid robot grid"
        );
        ensure!(
            self.cells.len() <= 65536
                && self
                    .cells
                    .iter()
                    .flatten()
                    .all(|n| (-500..=500).contains(n)),
            "robot volume exceeds bounds"
        );
        ensure!(
            self.compute_ms.is_finite() && self.compute_ms >= 0.,
            "invalid occupancy duration"
        );
        let covered: BTreeSet<_> = self.covered_links.iter().collect();
        ensure!(
            self.covered_links.len() + self.missing_links.len() <= 128
                && self.approximate_links.len() <= 128
                && self
                    .covered_links
                    .iter()
                    .chain(&self.missing_links)
                    .chain(&self.approximate_links)
                    .all(|s| !s.is_empty() && s.len() <= 128)
                && self.missing_links.iter().all(|s| !covered.contains(s)),
            "invalid body coverage"
        );
        Ok(())
    }
}

#[derive(Clone)]
pub struct Sample {
    pub reference: ProductReference,
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub clock: ClockReference,
    pub frame: auki_registry::FrameRegistryEntry,
    pub body: RobotOccupancy,
    pub received: Instant,
}
pub type Cache = Arc<Mutex<BTreeMap<String, VecDeque<Sample>>>>;

pub async fn monitor(
    peer: &auki_sdk::AukiPeer,
    targets: &[crate::network::HomeConnection],
    cache: &Cache,
    stop: &tokio_util::sync::CancellationToken,
) {
    futures::future::join_all(targets.iter().map(|target| async move {
        let work = async {
            let client = ComponentProtocolClient::new(peer.protocols());
            let expected: auki_sdk::PeerId = target.peer_id.parse()?;
            let mut product = None;
            let mut catalog_at = Instant::now() - Duration::from_secs(20);
            let mut timer = tokio::time::interval(Duration::from_millis(500));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { _=stop.cancelled()=>break, _=timer.tick()=>{} }
                if catalog_at.elapsed() > Duration::from_secs(10) {
                    catalog_at = Instant::now();
                    let mut routes: Vec<auki_sdk::Multiaddr> = target.routes.iter().filter_map(|r| r.parse().ok()).collect();
                    if routes.is_empty() && let Ok(Ok(peers)) = tokio::time::timeout(Duration::from_secs(2), peer.discover()).await {
                        for p in peers { if p.peer_id() == expected { routes.extend_from_slice(p.routes()); } }
                    }
                    for route in routes {
                        if let Ok(Ok(CatalogResponse::Snapshot { snapshot })) = tokio::time::timeout(Duration::from_secs(2), client.catalog_exact(expected, route.clone(), None)).await {
                            product = snapshot.products.iter().find(|p| p.manifest.peer_id == target.peer_id && p.manifest.product_id == "robot.occupancy-history"
                                && snapshot.components.iter().flat_map(|c|c.current_outputs.values()).any(|o| o.manifest.reference()==p.manifest.producer
                                    && matches!(&o.manifest.payload,PayloadContract::Structured(c) if c.datatype==RobotOccupancy::DATATYPE && c.schema=="spatial.robot-occupancy/v1")))
                                .map(|p| (ProductReference { peer_id: target.peer_id.clone(), product_id:p.manifest.product_id.clone(), manifest_hash:p.manifest_hash.clone() },route));
                            break;
                        }
                    }
                }
                let Some((reference, route)) = &product else {continue};
                let result = tokio::time::timeout(Duration::from_secs(2), client.observations_exact::<RobotOccupancy>(expected, route.clone(), ObservationRequest {product:reference.clone(),selection:ObservationSelection::LatestExisting})).await;
                let Ok(Ok(batch)) = result else {continue};
                if batch.product.reference()!=*reference || batch.producer.peer_id!=target.peer_id || batch.end.is_some() || batch.gap.is_some() {
                    cache.lock().unwrap().remove(&target.peer_id); product=None;continue;
                }
                let Some(observation)=batch.observations.last() else {continue};
                let Some(frame) = batch.producer.spatial_frame.as_ref() else {continue};
                let ros = auki_registry::FrameRegistryEntry::ros_body(&target.peer_id,"robot.base");
                let bracket = auki_registry::FrameRegistryEntry::in_convention(&target.peer_id,"robot.base",auki_registry::CoordinateConvention::ZUpRightForward);
                if observation.payload.validate().is_err() || frame.validate().is_err()
                    || (frame.definition != ros && frame.definition != bracket) {continue;}
                let mut cache=cache.lock().unwrap();
                // Re-reading the same observation must not refresh its expiry.
                let history=cache.entry(target.peer_id.clone()).or_default();
                if history.back().is_some_and(|s|s.reference==*reference && s.sequence>=observation.sequence) {continue;}
                if history.back().is_some_and(|s|s.reference!=*reference) {history.clear();}
                if history.len()==6 {history.pop_front();}
                history.push_back(Sample {reference:reference.clone(),sequence:observation.sequence,timestamp_ns:observation.timestamp_ns,
                    clock:batch.producer.clock.clone(),frame:frame.definition.clone(),body:(*observation.payload).clone(),received:Instant::now()});
            }
            Ok::<_,anyhow::Error>(())
        }.await;
        if let Err(e)=work {eprintln!("Robot occupancy {}: {e}",target.peer_id);}
    })).await;
}

pub fn render(
    sample: &Sample,
    frames: &crate::ar_preview::Frames,
    scene: &crate::network::CanonicalScene,
    home: bool,
) -> Result<Value> {
    ensure!(
        sample.received.elapsed() < Duration::from_secs(3),
        "Robot volume expired"
    );
    let peer = &sample.reference.peer_id;
    let local = &scene.registries["local_mapping"];
    let record = if home { &local["runners"][peer] } else { local };
    if home {
        ensure!(
            record["status"] == "aligned",
            "Robot is not aligned to Home"
        );
    } else {
        ensure!(
            scene.authority_peer_id == *peer,
            "Robot scene owner mismatch"
        );
    }
    let origin: crate::local_mapper::Origin = serde_json::from_value(record["origin"].clone())?;
    let cache = frames
        .lock()
        .map_err(|_| anyhow!("Pose cache unavailable"))?;
    let origin = if home {
        cache.alignment_at(peer, sample.timestamp_ns, &origin)?
    } else {
        origin
    };
    let history = cache
        .poses
        .get(peer)
        .ok_or_else(|| anyhow!("Robot odometry unavailable"))?;
    ensure!(
        history.contract.clock == sample.clock
            && history.contract.session_id == sample.body.session_id
            && history.contract.from_frame
                == sample.frame
            && origin.capture_clock == sample.clock
            && sample.timestamp_ns >= origin.capture_timestamp_ns,
        "Robot volume session mismatch"
    );
    let latest = history
        .product
        .latest_existing()?
        .ok_or_else(|| anyhow!("Waiting for robot odometry"))?;
    ensure!(
        latest.timestamp_ns.saturating_sub(sample.timestamp_ns) <= 3_000_000_000,
        "Robot volume is stale"
    );
    let base = auki_odometry::pose_at(
        &history.product,
        &auki_odometry::PoseQuery {
            contract: history.contract.clone(),
            timestamp_ns: sample.timestamp_ns,
            max_gap_ns: 200_000_000,
        },
    )?;
    let pose = crate::local_mapper::compose(&origin.odometry_to_local, &base.pose)?;
    let display = crate::local_mapper::display_frame(&scene.frame)?;
    let target = crate::conventions::registry(&scene.authority_peer_id, &display)?;
    let pose = crate::conventions::pose(&pose, &scene.voxels.definition.frame, &target)?;
    Ok(
        json!({"peer_id":peer,"frame":display,"pose":pose,"sequence":sample.sequence,"timestamp_ns":sample.timestamp_ns,
        "model_sha256":sample.body.model_sha256,"voxel_size":sample.body.voxel_size_m,"cells":sample.body.cells,
        "covered_links":sample.body.covered_links,"missing_links":sample.body.missing_links,"approximate_links":sample.body.approximate_links,
        "compute_ms":sample.body.compute_ms,"age_ms":sample.received.elapsed().as_millis()}),
    )
}

/// Independent streams can arrive out of order. Choose the newest body snapshot
/// with retained capture-time odometry rather than extrapolating or flashing off.
pub fn render_latest(
    samples: &VecDeque<Sample>,
    frames: &crate::ar_preview::Frames,
    scene: &crate::network::CanonicalScene,
    home: bool,
) -> Result<Value> {
    let mut failure = None;
    for sample in samples.iter().rev() {
        match render(sample, frames, scene, home) {
            Ok(value) => return Ok(value),
            Err(e) => {
                if failure.is_none() {
                    failure = Some(e);
                }
            }
        }
    }
    Err(failure.unwrap_or_else(|| anyhow!("Waiting for robot volume")))
}
