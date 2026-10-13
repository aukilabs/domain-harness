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
    datatype: String,
    pub(crate) id: String,
    pub(crate) reference: ProductReference,
    pub(crate) route: Option<Multiaddr>,
    pub(crate) peer: PeerId,
    pub(crate) metadata: Option<auki_components::CatalogProductMetadata>,
    pose: Option<ProductReference>,
}

/// Discover pose-bearing frames by SDK contracts, independent of camera streams.
pub fn frame_sensors(catalogs: &crate::device_catalogs::Catalogs) -> Vec<Sensor> {
    let mut found = Vec::new();
    for (owner,row) in catalogs {
        if row["catalog_status"] != "ready" { continue; }
        let (Ok(peer),Ok(catalog)) = (owner.parse::<PeerId>(),serde_json::from_value::<CatalogSnapshot>(row["catalog"].clone())) else { continue; };
        for p in &catalog.products {
            let Some(metadata) = &p.metadata else { continue; };
            let Ok(contract) = serde_json::from_value::<auki_odometry::PoseContract>(metadata.value.clone()) else { continue; };
            if p.manifest.peer_id != *owner || contract.from_frame.peer_id != *owner || contract.validate().is_err() { continue; }
            if !catalog.components.iter().flat_map(|c|c.current_outputs.values()).any(|o|
                o.manifest.peer_id == *owner && o.manifest.reference() == p.manifest.producer
                && o.manifest.clock == contract.clock
                && o.manifest.spatial_frame.as_ref().map(|f|&f.definition) == Some(&contract.to_frame)
                && matches!(&o.manifest.payload,PayloadContract::Structured(c) if c.datatype==auki_odometry::PoseUpdate::DATATYPE)) { continue; }
            found.push(Sensor { datatype:auki_odometry::PoseUpdate::DATATYPE.into(),id:format!("{owner}/{}",p.manifest.product_id),peer,
                reference:ProductReference{peer_id:owner.clone(),product_id:p.manifest.product_id.clone(),manifest_hash:p.manifest_hash.clone()},
                route:row["catalog_route"].as_str().and_then(|r|r.parse().ok()),metadata:Some(metadata.clone()),pose:None });
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
            let pose_id = p
                .metadata
                .as_ref()
                .and_then(|m| m.value["pose_product_id"].as_str());
            let pose = snapshot.products.iter().find(|g|
                g.manifest.peer_id == *owner && Some(g.manifest.product_id.as_str()) == pose_id &&
                snapshot.components.iter().flat_map(|c|c.current_outputs.values()).any(|o|
                    o.manifest.reference()==g.manifest.producer && matches!(&o.manifest.payload,
                        PayloadContract::Structured(c) if c.datatype==auki_odometry::PoseUpdate::DATATYPE))
            ).map(|g| ProductReference { peer_id:owner.clone(), product_id:g.manifest.product_id.clone(), manifest_hash:g.manifest_hash.clone() });
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
    observation: Observation<T>,
    producer: OutputManifest,
    received: Instant,
}
struct Preview<T = VideoFrame> {
    reference: ProductReference,
    used: Instant,
    frames: watch::Receiver<Option<Arc<Frame<T>>>>,
    task: tokio::task::JoinHandle<()>,
    ar_context: Option<crate::discovered_ar::Context>,
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
    let task = tokio::spawn(async move {
        let expected = sensor.reference.clone();
        let limits = BufferLimits {
            max_entries: Some(1),
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
        loop {
            match subscription.next().await {
                Ok(Some(RemoteObservationEvent::Observation(observation))) => {
                    send.send_replace(Some(Arc::new(Frame {
                        observation,
                        producer: producer.clone(),
                        received: Instant::now(),
                    })));
                }
                Ok(Some(RemoteObservationEvent::Gap(_))) => { send.send_replace(None); },
                _ => break,
            }
        }
    });
    Preview {
        reference,
        used: Instant::now(),
        frames,
        task,
        ar_context: None,
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
    home_live: tokio::sync::watch::Receiver<crate::scene_mirror::RemoteScene>,
    ar_frames: &crate::ar_preview::Frames,
    local_scenes: &crate::local_scenes::LocalScenes,
    stop: &tokio_util::sync::CancellationToken,
) {
    let client = ComponentProtocolClient::new(peer.protocols());
    let mut previews = BTreeMap::<String, Preview>::new();
    let mut following = BTreeMap::<String, Preview<auki_odometry::PoseUpdate>>::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        let sensors = camera_sensors(&catalogs.borrow());
        let frames = frame_sensors(&catalogs.borrow());
        following.retain(|id,p| p.used.elapsed()<PREVIEW_IDLE && !p.task.is_finished()
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
            if request.follow {
                let id=request.sensor.as_ref().ok_or_else(||anyhow!("Select a sensor frame"))?;
                let sensor=frames.iter().find(|s|&s.id==id).ok_or_else(||anyhow!("Sensor frame unavailable; waiting for its peer"))?;
                ensure!(request.expected_manifest.as_deref()==Some(sensor.reference.manifest_hash.as_str()),"Sensor Product changed; attach again");
                let preview=following.entry(id.clone()).or_insert_with(||start_preview(client.clone(),sensor.clone(),ObservationStart::NewOnly,|_:&auki_odometry::PoseUpdate|1024));
                preview.used=Instant::now();
                let sample=preview.frames.borrow().clone().ok_or_else(||anyhow!("Waiting for fresh sensor poses"))?;
                ensure!(sample.received.elapsed()<Duration::from_secs(2),"Sensor pose stream is stale");
                let contract:auki_odometry::PoseContract=serde_json::from_value(sensor.metadata.as_ref().unwrap().value.clone())?;
                ensure!(sample.observation.payload.contract==contract && sample.observation.output==sample.producer.reference()
                    && sample.producer.peer_id==sensor.reference.peer_id && sample.producer.clock==contract.clock
                    && sample.producer.spatial_frame.as_ref().map(|f|&f.definition)==Some(&contract.to_frame),"Sensor pose contract changed");
                let row=catalogs.borrow().get(&sensor.reference.peer_id).cloned().ok_or_else(||anyhow!("Peer unavailable"))?;
                let (frame,pose)=crate::discovered_ar::resolve_pose(&client,sensor,&row,&sample.observation,local_scenes,&peer.peer_id().to_string(),&mut preview.ar_context).await?;
                let mut value=crate::view_camera::frame_view(&pose,&contract.from_frame)?;
                value["frame"]=json!(frame);value["sensor_id"]=json!(id);value["manifest_hash"]=json!(sensor.reference.manifest_hash);
                value["sequence"]=json!(sample.observation.sequence);value["capture_timestamp_ns"]=json!(sample.observation.timestamp_ns);
                value["clock"]=json!(contract.clock);value["source_frame"]=json!(contract.from_frame);
                ensure!(sample.received.elapsed()<Duration::from_secs(2),"Sensor pose expired while resolving its transform");
                value["age_ms"]=json!(sample.received.elapsed().as_millis() as u64);
                return Ok(value);
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
                    let reference = sensor.pose.as_ref().unwrap();
                    let query = ObservationRequest {
                        product: reference.clone(),
                        selection: ObservationSelection::TimeRange {
                            request: auki_components::TimeRangeRequest {
                                clock: observations.producer.clock.clone(),
                                start_ns: o.timestamp_ns,
                                end_ns: o.timestamp_ns,
                            },
                        },
                    };
                    let pose = if let Some(route) = &sensor.route {
                        client
                            .observations_exact::<auki_odometry::PoseUpdate>(
                                sensor.peer,
                                route.clone(),
                                query,
                            )
                            .await?
                    } else {
                        client
                            .observations::<auki_odometry::PoseUpdate>(sensor.peer, query)
                            .await?
                    };
                    ensure!(
                        pose.product.reference() == *reference
                            && pose.gap.is_none()
                            && pose.end.is_none(),
                        "Camera pose Product changed or ended"
                    );
                    let g = pose
                        .observations
                        .iter()
                        .find(|g| g.timestamp_ns == o.timestamp_ns)
                        .ok_or_else(|| anyhow!("Waiting for capture-matched camera transform"))?;
                    let live = home_live.borrow().clone();
                    // Existing canonical mapper alignment remains supported, but is
                    // never a prerequisite for a discovered peer's own frame chain.
                    let legacy_aligned = live.status_view()["status"] == "connected"
                        && live.scene.as_ref().is_some_and(|s| s.registries["local_mapping"]["runners"][&sensor.reference.peer_id]["status"] == "aligned");
                    if !legacy_aligned {
                        let row = catalogs.borrow().get(&sensor.reference.peer_id).cloned()
                            .ok_or_else(|| anyhow!("Camera peer no longer discovered"))?;
                        return crate::discovered_ar::render(&client, sensor, &row, o, &observations.producer,
                            g, &pose.producer, local_scenes, &peer.peer_id().to_string(),
                            &mut preview.ar_context, request.ar).await;
                    }
                    ensure!(
                        live.status_view()["status"] == "connected",
                        "Home map unavailable"
                    );
                    crate::ar_preview::camera_pose_preview_mode(
                        &sensor.reference,
                        o,
                        &observations.producer,
                        g,
                        &pose.producer,
                        &sensor
                            .metadata
                            .as_ref()
                            .ok_or_else(|| anyhow!("Camera calibration metadata missing"))?
                            .value["calibration"],
                        ar_frames,
                        live.scene
                            .as_ref()
                            .ok_or_else(|| anyhow!("Home map unavailable"))?,
                        request.ar,
                    )
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
