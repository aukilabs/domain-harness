//! Demand-driven previews from typed Products in discovered SDK catalogs.
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{
    ComponentProtocolClient, ObservationRequest, ObservationSelection, ObservationStart,
    RemoteObservationEvent,
};
use auki_components::{
    BufferLimits, CatalogSnapshot, ContractType, Observation, OutputManifest, PayloadContract,
    ProductReference, VideoFrame,
};
use auki_sdk::{AukiPeer, Multiaddr, PeerId};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};
pub struct Request {
    pub home_frame: Option<String>,
    pub debug_pose: bool,
    pub sensor: Option<String>,
    pub scene: bool,
    pub ar: bool,
    pub pick: bool,
    pub follow: bool,
    pub capture: bool,
    pub expected_manifest: Option<String>,
    pub reply: oneshot::Sender<Result<Value>>,
}

// Polling an open PiP renews this lease. It must exceed the complete request
// budget, including queueing, and starts again when a request finishes.
const PREVIEW_IDLE: Duration = Duration::from_secs(30);
const POSE_WAIT: Duration = Duration::from_secs(2);
fn waiting_for_pose(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<auki_odometry::PoseError>(),
        Some(auki_odometry::PoseError::OutsideHistory))
        || error.to_string() == "Waiting for capture-matched camera transform"
        || error.to_string() == "Pose after capture unavailable; no extrapolation"
}

/// Discover scene products by SDK type and producer identity, not peer names.
pub fn scene_products(catalogs: &crate::device_catalogs::Catalogs) -> Vec<Sensor> {
    let mut found = Vec::new();
    for (owner, row) in catalogs {
        if row["catalog_status"] != "ready" {
            continue;
        }
        let Ok(peer) = owner.parse::<PeerId>() else {
            continue;
        };
        let Ok(snapshot) = serde_json::from_value::<CatalogSnapshot>(row["catalog"].clone()) else {
            continue;
        };
        for p in &snapshot.products {
            if p.manifest.peer_id != *owner {
                continue;
            }
            let datatype = snapshot
                .components
                .iter()
                .flat_map(|c| c.current_outputs.values())
                .find_map(|o| {
                    if o.manifest.peer_id != *owner || o.manifest.reference() != p.manifest.producer
                    {
                        return None;
                    }
                    match &o.manifest.payload {
                        PayloadContract::Structured(c)
                            if c.datatype == c.schema
                                && [
                                    crate::network::SCENE_SCHEMA,
                                    crate::occupancy_mapper::SCHEMA,
                                    crate::mobile_local::SCHEMA,
                                ]
                                .contains(&c.datatype.as_str()) =>
                        {
                            Some(c.datatype.clone())
                        }
                        _ => None,
                    }
                });
            let Some(datatype) = datatype else {
                continue;
            };
            found.push(Sensor {
                datatype,
                id: format!("{owner}/{}", p.manifest.product_id),
                peer,
                reference: ProductReference {
                    peer_id: owner.clone(),
                    product_id: p.manifest.product_id.clone(),
                    manifest_hash: p.manifest_hash.clone(),
                },
                route: row["catalog_route"].as_str().and_then(|r| r.parse().ok()),
                metadata: p.metadata.clone(),
                pose: None,
            });
        }
    }
    found
}

pub async fn read_scene(client: &ComponentProtocolClient, scene: &Sensor) -> Result<Value> {
    let query = ObservationRequest {
        product: scene.reference.clone(),
        selection: ObservationSelection::LatestExisting,
    };
    if scene.datatype == crate::mobile_local::SCHEMA {
        let response = if let Some(route) = &scene.route {
            client
                .observations_exact::<crate::mobile_local::Snapshot>(scene.peer, route.clone(), query)
                .await?
        } else {
            client
                .observations::<crate::mobile_local::Snapshot>(scene.peer, query)
                .await?
        };
        ensure!(
            response.product.reference() == scene.reference
                && response.gap.is_none()
                && response.end.is_none(),
            "Phone local map product changed or ended"
        );
        let observation = response.observations.last()
            .ok_or_else(|| anyhow!("Waiting for the phone's first local map"))?;
        let mut view = crate::mobile_local::render_snapshot(
            &observation.payload, &scene.reference.peer_id)?;
        view["product_id"] = json!(scene.reference.product_id);
        return Ok(view);
    }
    if scene.datatype == crate::occupancy_mapper::SCHEMA {
        let response = if let Some(route) = &scene.route {
            client
                .observations_exact::<crate::occupancy_mapper::OccupancyScene>(
                    scene.peer,
                    route.clone(),
                    query,
                )
                .await?
        } else {
            client
                .observations::<crate::occupancy_mapper::OccupancyScene>(scene.peer, query)
                .await?
        };
        ensure!(
            response.product.reference() == scene.reference
                && response.gap.is_none()
                && response.end.is_none(),
            "Occupancy product changed or ended"
        );
        let observation = response
            .observations
            .last()
            .ok_or_else(|| anyhow!("Waiting for occupancy map"))?;
        let mut view =
            crate::occupancy_mapper::render_scene(&observation.payload, &scene.reference.peer_id)?;
        view["product_id"] = json!(scene.reference.product_id);
        return Ok(view);
    }
    let response = if let Some(route) = &scene.route {
        client
            .observations_exact::<crate::network::CanonicalScene>(scene.peer, route.clone(), query)
            .await?
    } else {
        client
            .observations::<crate::network::CanonicalScene>(scene.peer, query)
            .await?
    };
    ensure!(
        response.product.reference() == scene.reference
            && response.gap.is_none()
            && response.end.is_none(),
        "Scene product changed or ended"
    );
    let observation = response
        .observations
        .last()
        .ok_or_else(|| anyhow!("Waiting for the first mapped scan"))?;
    let snapshot = &observation.payload;
    snapshot.validate(&scene.reference.peer_id)?;
    let mut view = crate::scene_mirror::render_scene(snapshot)?;
    view["authority"] = json!(scene.reference.peer_id);
    view["sources"] =
        json!({scene.reference.product_id.clone(): snapshot.voxels.integrated_observations});
    view["product_id"] = json!(scene.reference.product_id);
    Ok(view)
}
#[derive(Clone)]
pub struct Sensor {
    pub(crate) datatype: String,
    pub(crate) id: String,
    pub(crate) reference: ProductReference,
    pub(crate) route: Option<Multiaddr>,
    pub(crate) peer: PeerId,
    pub(crate) metadata: Option<auki_components::CatalogProductMetadata>,
    pub(crate) pose: Option<ProductReference>,
}

/// Discover pose-bearing frames by SDK contracts, independent of camera streams.
pub fn frame_sensors(catalogs: &crate::device_catalogs::Catalogs) -> Vec<Sensor> {
    // Camera following is not yet migrated; never advertise legacy poses.
    let _ = catalogs;
    Vec::new()
}
fn all_frame_sensors(catalogs: &crate::device_catalogs::Catalogs) -> Vec<Sensor> {
    let mut found = Vec::new();
    for (owner,row) in catalogs {
        if row["catalog_status"] != "ready" { continue; }
        let (Ok(peer),Ok(catalog)) = (owner.parse::<PeerId>(),serde_json::from_value::<CatalogSnapshot>(row["catalog"].clone())) else { continue; };
        for p in &catalog.products {
            let Some(metadata) = &p.metadata else { continue; };
            if metadata.schema==crate::compact_pose::METADATA {
                let Ok(contract)=serde_json::from_value::<crate::compact_pose::Contract>(metadata.value.clone()) else {continue};
                if p.manifest.peer_id!=*owner || !catalog.components.iter().flat_map(|c|c.current_outputs.values())
                    .any(|o|o.manifest.reference()==p.manifest.producer && contract.validate_output(&o.manifest).is_ok()) {continue}
                found.push(Sensor {datatype:crate::compact_pose::SCHEMA.into(),id:format!("{owner}/{}",p.manifest.product_id),peer,
                    reference:p.manifest.reference(),route:row["catalog_route"].as_str().and_then(|r|r.parse().ok()),metadata:Some(metadata.clone()),pose:None});
                continue;
            }
        }
    }
    found
}

pub fn camera_sensors(catalogs: &crate::device_catalogs::Catalogs) -> Vec<Sensor> {
    let mut found = Vec::new();
    for (owner, row) in catalogs {
        if row["catalog_status"] != "ready" {
            continue;
        }
        let Ok(expected) = owner.parse::<PeerId>() else {
            continue;
        };
        let Ok(snapshot) = serde_json::from_value::<CatalogSnapshot>(row["catalog"].clone()) else {
            continue;
        };
        let route = row["catalog_route"].as_str().and_then(|r| r.parse().ok());
        for p in &snapshot.products {
            if p.manifest.peer_id != *owner {
                continue;
            }
            if !snapshot.components.iter().flat_map(|c|c.current_outputs.values()).any(|o|
                o.manifest.peer_id == *owner && o.manifest.reference()==p.manifest.producer &&
                matches!(&o.manifest.payload, PayloadContract::Camera(c) if c.datatype==VideoFrame::DATATYPE)) { continue; }
            let pose = snapshot.components.iter().flat_map(|c| c.current_outputs.values())
                .find(|o| o.manifest.reference() == p.manifest.producer)
                .and_then(|o| crate::compact_ar::candidate(&snapshot, &o.manifest).ok())
                .map(|(reference, _)| reference);
            found.push(Sensor {
                datatype: VideoFrame::DATATYPE.into(),
                id: format!("{owner}/{}", p.manifest.product_id),
                peer: expected,
                reference: ProductReference {
                    peer_id: owner.clone(),
                    product_id: p.manifest.product_id.clone(),
                    manifest_hash: p.manifest_hash.clone(),
                },
                route: route.clone(),
                metadata: p.metadata.clone(),
                pose,
            });
        }
    }
    found
}

/// Same typed camera identities used by the PiP browser, without opening streams.
pub fn camera_choices(catalogs: &crate::device_catalogs::Catalogs) -> Vec<Value> {
    camera_sensors(catalogs).iter().map(|s|json!({"id":s.id,"peer_id":s.reference.peer_id,"product_id":s.reference.product_id})).collect()
}

struct Frame<T = VideoFrame> {
    received_hz: f64,
    observation: Observation<T>,
    producer: OutputManifest,
    received: Instant,
}
struct Preview<T = VideoFrame> {
    history: Arc<std::sync::Mutex<std::collections::VecDeque<Arc<Frame<T>>>>>,
    resolved_pose: Option<Value>,
    reference: ProductReference,
    used: Instant,
    frames: watch::Receiver<Option<Arc<Frame<T>>>>,
    task: tokio::task::JoinHandle<()>,
}
impl<T> Drop for Preview<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn start_preview<T: ContractType + serde::de::DeserializeOwned + Send + Sync + 'static>(client: ComponentProtocolClient, sensor: Sensor,
    start: ObservationStart, size: fn(&T)->usize) -> Preview<T> {
    let (send, frames) = watch::channel(None);
    let reference = sensor.reference.clone();
    let log_poses = sensor.datatype == crate::compact_pose::SCHEMA;
    let history = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    let received_history = history.clone();
    let task = tokio::spawn(async move {
        let expected = sensor.reference.clone();
        let limits = BufferLimits {
            max_entries: Some(if log_poses { 300 } else { 1 }),
            max_bytes: Some(8 * 1024 * 1024),
            target_duration: None,
        };
        let connect = async {
            if let Some(route) = sensor.route {
                client
                    .subscribe_product_exact(
                        sensor.peer,
                        route,
                        sensor.reference,
                        start,
                        limits,
                        size,
                    )
                    .await
            } else {
                client
                    .subscribe_product(
                        sensor.peer,
                        sensor.reference,
                        start,
                        limits,
                        size,
                    )
                    .await
            }
        };
        let Ok(Ok(mut subscription)) = tokio::time::timeout(Duration::from_secs(5), connect).await
        else {
            return;
        };
        let producer = subscription.product().producer.clone();
        if subscription.product().reference() != expected || producer.peer_id != expected.peer_id { return; }
        let mut arrivals = std::collections::VecDeque::new();
        loop {
            match subscription.next().await {
                Ok(Some(RemoteObservationEvent::Observation(observation))) => {
                    let now = Instant::now();
                    arrivals.push_back(now);
                    while arrivals.len()>2000 || arrivals.front().is_some_and(|t| now.duration_since(*t)>Duration::from_secs(2)) { arrivals.pop_front(); }
                    let elapsed = arrivals.front().map(|t|now.duration_since(*t).as_secs_f64()).unwrap_or(0.0);
                    let received_hz = if elapsed>0.0 {(arrivals.len()-1) as f64/elapsed} else {0.0};
                    let frame = Arc::new(Frame {
                        received_hz,
                        observation,
                        producer: producer.clone(),
                        received: Instant::now(),
                    });
                    if log_poses {
                        let mut history = received_history.lock().unwrap();
                        history.push_back(frame.clone());
                        while history.len() > 300 { history.pop_front(); }
                    }
                    send.send_replace(Some(frame));
                }
                Ok(Some(RemoteObservationEvent::Gap(_))) => { send.send_replace(None); },
                _ => break,
            }
        }
    });
    Preview {
        history,
        resolved_pose: None,
        reference,
        used: Instant::now(),
        frames,
        task,
    }
}
async fn capture_frame(client:ComponentProtocolClient,sensor:Sensor)->Result<Value>{
    // NewOnly excludes retained history. Drop cancels on success/error/timeout.
    let mut capture=start_preview(client,sensor.clone(),ObservationStart::NewOnly,|f:&VideoFrame|f.bytes.len());
    capture.frames.changed().await.map_err(|_|anyhow!("Camera stopped before publishing a fresh frame"))?;
    let sample=capture.frames.borrow().clone().ok_or_else(||anyhow!("Camera stream has a gap"))?;
    ensure!(sample.received.elapsed()<Duration::from_secs(2),"Camera frame is stale");
    ensure!(sample.observation.output==sample.producer.reference(),"Camera output changed");
    let image=crate::camera_vision::encode(&sample.observation.payload)?;
    Ok(json!({"image":image,"peer_id":sensor.reference.peer_id,"product":sensor.reference,
        "sequence":sample.observation.sequence,"capture_timestamp_ns":sample.observation.timestamp_ns,
        "clock":sample.producer.clock,"width":sample.observation.payload.width,"height":sample.observation.payload.height}))
}

pub async fn serve(
    peer: &AukiPeer,
    catalogs: watch::Receiver<crate::device_catalogs::Catalogs>,
    mut requests: mpsc::Receiver<Request>,
    _home_live: tokio::sync::watch::Receiver<crate::scene_mirror::RemoteScene>,
    _ar_frames: &crate::ar_preview::Frames,
    local_scenes: &crate::local_scenes::LocalScenes,
    stop: &tokio_util::sync::CancellationToken,
) {
    let client = ComponentProtocolClient::new(peer.protocols());
    let mut previews = BTreeMap::<String, Preview>::new();
    let mut compact = BTreeMap::<String, Preview<crate::compact_pose::Update>>::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        let sensors = camera_sensors(&catalogs.borrow());
        let frames = all_frame_sensors(&catalogs.borrow());
        compact.retain(|id,p|p.used.elapsed()<PREVIEW_IDLE && !p.task.is_finished()
            && frames.iter().any(|s|s.id==*id && s.reference==p.reference));
        previews.retain(|id, p| {
            p.used.elapsed() < PREVIEW_IDLE
                && !p.task.is_finished()
                && sensors.iter().find(|s| s.id == *id)
                    .is_none_or(|s| s.reference == p.reference)
        });
        let request = tokio::select! {_ = stop.cancelled()=>break, _ = tick.tick()=>continue, r=requests.recv()=>match r {Some(r)=>r,None=>break}};
        let sensors = camera_sensors(&catalogs.borrow());
        if request.capture {
            // This internal one-shot request comes from the serialized chat gate.
            // Waiting for its frame must not block open PiP/follow requests.
            let sensor=sensors.iter().find(|s|Some(&s.id)==request.sensor.as_ref()).cloned();
            let client=client.clone();let stop=stop.clone();
            tokio::spawn(async move {
                let work=async {capture_frame(client,sensor.ok_or_else(||anyhow!("Sensor unavailable; waiting for its peer"))?).await};
                let result=tokio::select!{_ = stop.cancelled()=>Err(anyhow!("Home stopped")),r=tokio::time::timeout(Duration::from_secs(8),work)=>r.unwrap_or_else(|_|Err(anyhow!("Camera did not publish a fresh frame in time")))};
                let _=request.reply.send(result);
            });
            continue;
        }
        let work = async {
            if request.follow || request.debug_pose {
                let id=request.sensor.as_ref().ok_or_else(||anyhow!("Select a sensor frame"))?;
                let sensor=frames.iter().find(|s|&s.id==id).ok_or_else(||anyhow!("Sensor frame unavailable; waiting for its peer"))?;
                ensure!(request.expected_manifest.as_deref()==Some(sensor.reference.manifest_hash.as_str()),"Sensor Product changed; attach again");
                if sensor.datatype==crate::compact_pose::SCHEMA {
                    ensure!(request.debug_pose,"Compact poses currently support inspection, not camera following");
                    let contract:crate::compact_pose::Contract=serde_json::from_value(sensor.metadata.as_ref().unwrap().value.clone())?;
                    let start=if contract.lifetime==crate::compact_pose::Lifetime::Held {ObservationStart::LatestExisting}else{ObservationStart::NewOnly};
                    let preview=compact.entry(id.clone()).or_insert_with(||start_preview(client.clone(),sensor.clone(),start,|_:&crate::compact_pose::Update|256));
                    preview.used=Instant::now();
                    let sample=preview.frames.borrow().clone().ok_or_else(||anyhow!("Waiting for fresh sensor poses"))?;
                    contract.validate_output(&sample.producer)?;
                    ensure!(sample.observation.output==sample.producer.reference(),"Pose producer mismatch");
                    sample.observation.payload.validate()?;
                    let row=catalogs.borrow().get(&sensor.reference.peer_id).cloned().ok_or_else(||anyhow!("Peer catalog unavailable"))?;
                    if preview.resolved_pose.is_none() {
                        let (resolved,clock)=crate::product_registry::resolved_contract(&row["registries"],&contract)?;
                        ensure!(sample.producer.spatial_frame.as_ref().is_some_and(|f| f.definition==resolved.to_frame),"Registry frame differs from output");
                        preview.resolved_pose=Some(json!({"contract":resolved,"clock_definition":clock,"references":contract}));
                    }
                    // Registry resolution can take time: re-read the most recent sample.
                    let sample=preview.frames.borrow().clone().ok_or_else(||anyhow!("Pose stream gap"))?;
                    sample.observation.payload.validate()?;
                    ensure!(sample.observation.output==sample.producer.reference(),"Pose output changed");
                    let home_pose=if let Some(target)=&request.home_frame {
                        let view=local_scenes.snapshots.keys().filter_map(|id|local_scenes.view(id).ok()).find(|v|v["frame"]==*target).ok_or_else(||anyhow!("Unknown Home scene frame"))?;
                        let (alignment,after)=crate::frame_alignment::resolve(&client,sensor,&row,&contract,&peer.peer_id().to_string(),&view,sample.observation.timestamp_ns).await?;
                        ensure!(sample.observation.timestamp_ns>=after,"Pose predates alignment");
                        let tracking=crate::frame_alignment::map_tracking(&sample.observation.payload,&alignment,sample.received.elapsed())?;
                        Some(json!({"frame":target,"tracking":tracking,"alignment_timestamp_ns":after.to_string()}))
                    }else{None};
                    let age=sample.received.elapsed();
                    let history=preview.history.lock().unwrap().iter().map(|frame| {
                        contract.validate_output(&frame.producer)?;
                        ensure!(frame.observation.output==frame.producer.reference(),"Pose producer mismatch");
                        frame.observation.payload.validate()?;
                        Ok(json!({"observation":frame.observation,
                            "sequence":frame.observation.sequence.to_string(),
                            "timestamp_ns":frame.observation.timestamp_ns.to_string()}))
                    }).collect::<Result<Vec<Value>>>()?;
                    return Ok(json!({"stream":if contract.lifetime==crate::compact_pose::Lifetime::Held {"held"} else if age<Duration::from_secs(2) {"live"} else {"stale"},
                        "home_pose":home_pose,
                        "history":history,"history_limit":300,
                        "age_ms":age.as_millis() as u64,"received_hz":if age<Duration::from_secs(2) {sample.received_hz} else {0.0},
                        "resolved":preview.resolved_pose,"observation":sample.observation,
                        "timestamp_ns":sample.observation.timestamp_ns.to_string(),"sequence":sample.observation.sequence.to_string()}));
                }
                anyhow::bail!("Unsupported pose contract; compact registry-backed poses required");
            }
            if request.scene {
                let scenes = scene_products(&catalogs.borrow());
                let Some(id) = &request.sensor else {
                    return Ok(
                        json!({"scenes":scenes.iter().map(|s|json!({"id":format!("peer-scene:{}",s.id),"name":s.reference.product_id,"peer_id":s.reference.peer_id})).collect::<Vec<_>>()}),
                    );
                };
                let scene = scenes
                    .iter()
                    .find(|s| &s.id == id)
                    .ok_or_else(|| anyhow!("Scene publisher unavailable"))?;
                return read_scene(&client, scene).await;
            }
            let Some(id) = &request.sensor else {
                return Ok(
                    json!({"sensors":sensors.iter().map(|s|json!({"id":s.id,"label":s.reference.product_id,"peer_id":s.reference.peer_id,"ar_available":s.pose.is_some()})).collect::<Vec<_>>()}),
                );
            };
            let sensor = sensors
                .iter()
                .find(|s| &s.id == id)
                .ok_or_else(|| anyhow!("Sensor unavailable; waiting for its peer"))?;
            if request.ar {
                ensure!(
                    sensor.pose.is_some(),
                    "Camera capture-time pose unavailable"
                );
            }
            let preview = previews
                .entry(id.clone())
                .or_insert_with(|| start_preview(client.clone(), sensor.clone(),ObservationStart::LatestExisting,|f:&VideoFrame|f.bytes.len()));
            preview.used = Instant::now();
            let observations = preview
                .frames
                .borrow()
                .clone()
                .ok_or_else(|| anyhow!("Waiting for camera frames"))?;
            let o = &observations.observation;
            ensure!(
                observations.received.elapsed() < Duration::from_secs(2),
                "Camera frames are stale"
            );
            let f = &o.payload;
            ensure!(
                f.width > 0
                    && f.height > 0
                    && f.width <= 4096
                    && f.height <= 4096
                    && f.bytes.len() <= 8 * 1024 * 1024,
                "Camera frame exceeds preview limits"
            );
            ensure!(
                matches!(f.encoding.as_str(), "jpeg" | "jpg" | "luma8" | "rgb8"),
                "Unsupported camera encoding: {}",
                f.encoding
            );
            let mut pick_error = None;
            if request.ar || (request.pick && sensor.pose.is_some()) {
                let deadline = tokio::time::Instant::now() + POSE_WAIT;
                let calibrated = loop {
                let attempt = async {
                    let row = catalogs.borrow().get(&sensor.reference.peer_id).cloned()
                        .ok_or_else(|| anyhow!("Camera peer no longer discovered"))?;
                    crate::compact_ar::render(&client, sensor, &row, o, &observations.producer,
                        local_scenes, &peer.peer_id().to_string(), request.ar).await
                }
                ;
                let result = tokio::time::timeout_at(deadline, attempt).await
                    .unwrap_or_else(|_| Err(anyhow!("Waiting for capture-matched poses; synchronization timed out")));
                match result {
                    Err(ref error) if waiting_for_pose(error) && tokio::time::Instant::now() < deadline => {
                        tokio::time::sleep_until((tokio::time::Instant::now() + Duration::from_millis(40)).min(deadline)).await;
                    }
                    Err(error) if waiting_for_pose(&error) => break Err(anyhow!("Waiting for capture-matched poses; synchronization timed out")),
                    result => break result,
                }
                };
                match calibrated {
                    Ok(frame) => return Ok(frame),
                    Err(e) if request.ar => return Err(e),
                    Err(e) => pick_error = Some(e.to_string()),
                }
            }
            Ok(
                json!({"pick_error":pick_error,"spatial_frame":observations.producer.spatial_frame,"metadata":sensor.metadata,"sequence":o.sequence,"capture_timestamp_ns":o.timestamp_ns,"clock":observations.producer.clock,"width":f.width,"height":f.height,"encoding":f.encoding,"data":STANDARD.encode(&f.bytes)}),
            )
        };
        let result = tokio::select! {_ = stop.cancelled()=>break,r=tokio::time::timeout(Duration::from_secs(8),work)=>r.unwrap_or_else(|_|Err(anyhow!("Sensor peer timed out")))};
        if let Some(preview) = request.sensor.as_ref().and_then(|id| previews.get_mut(id)) {
            preview.used = Instant::now();
        }
        let _ = request.reply.send(result);
    }
}

#[cfg(test)]
mod preview_timing_tests {
    use super::*;
    #[test]
    fn wait_only_for_missing_history_not_invalid_tracking() {
        assert!(waiting_for_pose(&anyhow::Error::new(auki_odometry::PoseError::OutsideHistory)));
        assert!(waiting_for_pose(&anyhow!("Waiting for capture-matched camera transform")));
        assert!(!waiting_for_pose(&anyhow::Error::new(auki_odometry::PoseError::TrackingLost)));
        assert!(!waiting_for_pose(&anyhow::Error::new(auki_odometry::PoseError::ContractMismatch)));
        assert!(!waiting_for_pose(&anyhow::Error::new(auki_odometry::PoseError::GapTooLarge)));
    }
    #[test]
    fn preview_lease_outlasts_request_processing() {
        assert!(PREVIEW_IDLE > Duration::from_secs(8) + POSE_WAIT);
    }
}
