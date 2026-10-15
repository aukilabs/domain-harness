//! Demand-driven previews from typed Products in discovered SDK catalogs.
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{
    ComponentProtocolClient, ObservationDecoder, ObservationRequest, ObservationSelection, ObservationStart,
    RemoteObservationEvent,
};
use auki_components::{
    CatalogSnapshot, ContractType, Observation, OutputManifest, PayloadContract,
    ProductReference, VideoFrame,
};
use auki_sdk::{AukiPeer, Multiaddr, PeerId};
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
    pub reply: oneshot::Sender<Result<Reply>>,
}

#[derive(Debug)]
pub struct Reply {pub value:Value,pub image:Option<Arc<[u8]>>}
impl From<Value> for Reply {fn from(value:Value)->Self {Self {value,image:None}}}
impl std::ops::Deref for Reply {type Target=Value;fn deref(&self)->&Value {&self.value}}

// Polling an open PiP renews this lease. It must exceed the complete request
// budget, including queueing, and starts again when a request finishes.
const PREVIEW_IDLE: Duration = Duration::from_secs(30);
const POSE_WAIT: Duration = Duration::from_secs(2);
fn accept_sequence(last: &mut Option<u64>, sequence: u64) -> bool {
    if last.is_some_and(|previous| sequence <= previous) { return false; }
    *last = Some(sequence);
    true
}
fn waiting_for_pose(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<auki_odometry::PoseError>(),
        Some(auki_odometry::PoseError::OutsideHistory))
        || error.to_string() == "Waiting for capture-matched camera transform"
        || error.to_string() == "Pose after capture unavailable; no extrapolation"
        || matches!(error.to_string().as_str(), "Held transform subscription pending"
            | "Held transform synchronizing" | "Held transform has not reached advertised sequence"
            | "Time-transform subscription synchronizing" | "Waiting for capture-matched time transforms"
            | "Waiting for pose before capture uncertainty interval"
            | "Waiting for pose after capture uncertainty interval"
            | "Waiting for validated poses across capture uncertainty interval")
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
                ar_supported: false,
            });
        }
    }
    found
}

pub async fn read_scene(client: &ComponentProtocolClient, scene: &Sensor) -> Result<Value> {
    read_scene_retained(client,scene,&mut crate::observation_history::Fetched::default()).await
}
fn scene_observation<T>(product: &ProductReference, output: &OutputManifest, observation: &Observation<T>) -> Value {
    json!({"product":product,"output":observation.output,"clock":output.clock,
        "sequence":observation.sequence.to_string(),"timestamp_ns":observation.timestamp_ns.to_string()})
}
async fn read_scene_retained(client: &ComponentProtocolClient, scene: &Sensor,
    history: &mut crate::observation_history::Fetched) -> Result<Value> {
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
        history.retain(&response)?;
        let observation = response.observations.last()
            .ok_or_else(|| anyhow!("Waiting for the phone's first local map"))?;
        let mut view = crate::mobile_local::render_snapshot(
            &observation.payload, &scene.reference.peer_id)?;
        view["product_id"] = json!(scene.reference.product_id);
        view["source_observation"] = scene_observation(&scene.reference, &response.producer, observation);
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
        history.retain(&response)?;
        let observation = response
            .observations
            .last()
            .ok_or_else(|| anyhow!("Waiting for occupancy map"))?;
        let mut view =
            crate::occupancy_mapper::render_scene(&observation.payload, &scene.reference.peer_id)?;
        view["product_id"] = json!(scene.reference.product_id);
        view["source_observation"] = scene_observation(&scene.reference, &response.producer, observation);
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
    history.retain(&response)?;
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
    view["source_observation"] = scene_observation(&scene.reference, &response.producer, observation);
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
    pub(crate) ar_supported: bool,
}

/// Discover pose-bearing frames by SDK contracts, independent of camera streams.
pub fn frame_sensors(catalogs: &crate::device_catalogs::Catalogs) -> Vec<Sensor> {
    all_frame_sensors(catalogs).into_iter().filter(|s|s.metadata.as_ref().is_some_and(|m|
        m.value["lifetime"].as_str().unwrap_or("sampled")=="sampled")).collect()
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
                    reference:p.manifest.reference(),route:row["catalog_route"].as_str().and_then(|r|r.parse().ok()),metadata:Some(metadata.clone()),ar_supported:false});
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
            let Some(camera)=snapshot.components.iter().flat_map(|c|c.current_outputs.values()).find_map(|o| {
                if o.manifest.peer_id != *owner || o.manifest.reference()!=p.manifest.producer {return None;}
                match &o.manifest.payload {PayloadContract::Camera(c) if c.datatype==VideoFrame::DATATYPE
                    && crate::camera_frame::supported(&c.schema)=>Some(c),_=>None}
            }) else {continue;};
            let ar_supported = crate::registry_ar::supported(p, &snapshot, &row["registries"]);
            found.push(Sensor {
                datatype: camera.schema.clone(),
                id: format!("{owner}/{}", p.manifest.product_id),
                peer: expected,
                reference: ProductReference {
                    peer_id: owner.clone(),
                    product_id: p.manifest.product_id.clone(),
                    manifest_hash: p.manifest_hash.clone(),
                },
                route: route.clone(),
                metadata: p.metadata.clone(),
                ar_supported,
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
    retained: Arc<std::sync::Mutex<Option<auki_components::RetainedProduct<T>>>>,
    presentation: Option<(u64,Value)>,
    reference: ProductReference,
    used: Instant,
    frames: watch::Receiver<Option<Arc<Frame<T>>>>,
    health: watch::Receiver<String>,
    task: tokio::task::JoinHandle<()>,
}
impl<T> Drop for Preview<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn start_preview<T: ContractType + serde::de::DeserializeOwned + Send + Sync + 'static>(client: ComponentProtocolClient, sensor: Sensor,
    start: ObservationStart, size: fn(&T)->usize, decoder:ObservationDecoder<T>) -> Preview<T> {
    let (send, frames) = watch::channel(None);
    let (health_send, health) = watch::channel("Connecting camera subscription".to_string());
    let reference = sensor.reference.clone();
    let retained = Arc::new(std::sync::Mutex::new(None));
    let retained_worker = retained.clone();
    let task = tokio::spawn(async move {
        let expected = sensor.reference.clone();
        let limits = crate::observation_history::cameras();
        let mut retry_ms = 250u64;
        let mut last_sequence = None;
        let mut last_gap_log = None::<Instant>;
        loop {
        let connect = async {
            if let Some(route) = sensor.route.clone() {
                client
                    .subscribe_product_exact_with_decoder(
                        sensor.peer,
                        route,
                        sensor.reference.clone(),
                        start,
                        limits,
                        size,
                        decoder.clone(),
                    )
                    .await
            } else {
                client
                    .subscribe_product_with_decoder(
                        sensor.peer,
                        sensor.reference.clone(),
                        start,
                        limits,
                        size,
                        decoder.clone(),
                    )
                    .await
            }
        };
        let mut subscription = match tokio::time::timeout(Duration::from_secs(5), connect).await {
            Ok(Ok(subscription)) => subscription,
            result => {
                let reason = match result { Ok(Err(e)) => e.to_string(), _ => "subscribe timed out".into() };
                health_send.send_replace(format!("Reconnecting camera: {reason}"));
                eprintln!("preview subscribe failed peer={} product={:?}: {reason}; retry_ms={retry_ms}", expected.peer_id, expected);
                tokio::time::sleep(Duration::from_millis(retry_ms)).await;
                retry_ms = (retry_ms * 2).min(5000);
                continue;
            }
        };
        let producer = subscription.product().producer.clone();
        if subscription.product().reference() != expected || producer.peer_id != expected.peer_id { return; }
        *retained_worker.lock().unwrap() = Some(subscription.product().clone());
        let mut arrivals = std::collections::VecDeque::new();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), subscription.next()).await;
            match event {
                Err(_) => { health_send.send_replace("Reconnecting camera: no stream event for 5s".into()); eprintln!("preview stalled peer={} product={:?}: no stream event for 5s", expected.peer_id, expected); break; }
                Ok(event) => match event {
                Ok(Some(RemoteObservationEvent::Observation(observation))) => {
                    // LatestExisting can replay the retained sample on reconnect.
                    // Never give that sample a new arrival time or enable stale AR.
                    if !accept_sequence(&mut last_sequence, observation.sequence) { continue; }
                    retry_ms = 250;
                    health_send.send_replace("Receiving camera frames".into());
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
                    send.send_replace(Some(frame));
                }
                Ok(Some(RemoteObservationEvent::Gap(gap))) => {
                    if last_gap_log.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                        eprintln!("preview retention gap peer={} product={:?}: {gap:?}", expected.peer_id, expected);
                        last_gap_log = Some(Instant::now());
                    }
                },
                Ok(Some(RemoteObservationEvent::Closed(end))) => { health_send.send_replace("Reconnecting camera: publisher closed stream".into()); eprintln!("preview closed peer={} product={:?}: {end:?}", expected.peer_id, expected); break; },
                Ok(None) => { health_send.send_replace("Reconnecting camera: end of stream".into()); eprintln!("preview EOF peer={} product={:?}", expected.peer_id, expected); break; },
                Err(error) => { health_send.send_replace(format!("Reconnecting camera: {error}")); eprintln!("preview stream failed peer={} product={:?}: {error}", expected.peer_id, expected); break; },
                }
            }
        }
        // Dropping the exact subscription cancels its stream before reopening.
        drop(subscription);
        tokio::time::sleep(Duration::from_millis(retry_ms)).await;
        retry_ms = (retry_ms * 2).min(5000);
        }
    });
    Preview {
        retained,
        presentation: None,
        reference,
        used: Instant::now(),
        frames,
        health,
        task,
    }
}
async fn capture_frame(sensor:&Sensor,frames:&mut watch::Receiver<Option<Arc<Frame>>>,after:Instant)->Result<Value>{
    // An already-open PiP is the source of truth. Its cached image is not a
    // fresh snapshot: wait for a new arrival after this request instead.
    let sample=loop {
        let sample=frames.borrow_and_update().clone();
        if let Some(sample)=sample.filter(|sample|sample.received>=after) {break sample;}
        frames.changed().await.map_err(|_|anyhow!("Camera stopped before publishing a fresh frame"))?;
    };
    ensure!(sample.received.elapsed()<Duration::from_secs(2),"Camera frame is stale");
    ensure!(sample.observation.output==sample.producer.reference(),"Camera output changed");
    crate::camera_frame::validate_output(&sample.observation.payload,&sample.producer)?;
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
    held: &crate::frame_alignment::HeldTransforms,
    stop: &tokio_util::sync::CancellationToken,
) {
    let client = ComponentProtocolClient::new(peer.protocols());
    let mut previews = BTreeMap::<String, Preview>::new();
    let mut fetched_history = crate::observation_history::Fetched::default();
    let camera_history = Arc::new(std::sync::Mutex::new(crate::observation_history::Cameras::default()));
    let mut captures = tokio::task::JoinSet::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        camera_history.lock().unwrap().expire();
        let sensors = camera_sensors(&catalogs.borrow());
        let frames = all_frame_sensors(&catalogs.borrow());
        previews.retain(|id, p| {
            p.used.elapsed() < PREVIEW_IDLE
                && !p.task.is_finished()
                && sensors.iter().find(|s| s.id == *id)
                    .is_none_or(|s| s.reference == p.reference)
        });
        // Active PiPs and one-shot snapshots share the same eight-camera
        // budget. Keep live reservations while their preview lease is active.
        for preview in previews.values() {
            let _=camera_history.lock().unwrap().reserve(&preview.reference);
        }
        let request = tokio::select! {_ = stop.cancelled()=>break, _ = tick.tick()=>continue,
            _ = captures.join_next(), if !captures.is_empty()=>continue,
            r=requests.recv()=>match r {Some(r)=>r,None=>break}};
        let sensors = camera_sensors(&catalogs.borrow());
        if request.capture {
            // Waiting for a snapshot must not block PiP/follow requests or open
            // another subscription when this camera already has a live preview.
            let after=Instant::now();
            let setup=(||->Result<_>{
                ensure!(captures.len()<8,"Home camera snapshot request limit reached");
                let sensor=sensors.iter().find(|s|Some(&s.id)==request.sensor.as_ref()).cloned()
                    .ok_or_else(||anyhow!("Sensor unavailable; waiting for its peer"))?;
                camera_history.lock().unwrap().reserve(&sensor.reference)?;
                let one_shot=if previews.contains_key(&sensor.id) {None}else {
                    let decoder=crate::camera_frame::decoder(&sensor.datatype)?;
                    Some(start_preview(client.clone(),sensor.clone(),ObservationStart::NewOnly,|f:&VideoFrame|f.bytes.len(),decoder))
                };
                let preview:&Preview=if let Some(preview)=previews.get_mut(&sensor.id) {
                    preview.used=after;preview
                }else{one_shot.as_ref().unwrap()};
                Ok((sensor,preview.frames.clone(),preview.retained.clone(),one_shot))
            })();
            let (sensor,mut frames,retained,one_shot)=match setup {
                Ok(source)=>source,
                Err(error)=>{let _=request.reply.send(Err(error));continue;}
            };
            let stop=stop.clone();let camera_history=camera_history.clone();
            captures.spawn(async move {
                let one_shot_capture=one_shot.is_some();
                let work=capture_frame(&sensor,&mut frames,after);
                let mut result=tokio::select!{_ = stop.cancelled()=>Err(anyhow!("Home stopped")),r=tokio::time::timeout(Duration::from_secs(8),work)=>r.unwrap_or_else(|_|Err(anyhow!("Camera did not publish a fresh frame in time")))};
                if let Some(mut preview)=one_shot {
                    preview.task.abort();
                    let _=(&mut preview.task).await;
                }
                // Preserve every consumed SDK observation even if encoding or
                // waiting failed, after closing the one-shot subscription.
                if let Some(product)=retained.lock().unwrap().clone() {
                    let coverage=if one_shot_capture {camera_history.lock().unwrap().retain(&product)}
                        else {Ok(crate::observation_history::coverage(product.buffer().snapshot(0,u64::MAX).iter().map(|o|o.payload.timestamp_ns)))};
                    match coverage {
                        Ok(coverage)=>if let Ok(value)=&mut result {value["retention"]=coverage;},
                        Err(error)=>if result.is_ok() {result=Err(error);},
                    }
                }
                let _=request.reply.send(result.map(Into::into));
            });
            continue;
        }
        let mut image=None;
        let work = async {
            if request.debug_pose || (request.follow && !sensors.iter().any(|s|Some(&s.id)==request.sensor.as_ref())) {
                let id=request.sensor.as_ref().ok_or_else(||anyhow!("Select a sensor frame"))?;
                let sensor=frames.iter().find(|s|&s.id==id).ok_or_else(||anyhow!("Sensor frame unavailable; waiting for its peer"))?;
                ensure!(request.expected_manifest.as_deref()==Some(sensor.reference.manifest_hash.as_str()),"Sensor Product changed; attach again");
                if sensor.datatype==crate::compact_pose::SCHEMA {
                    let contract:crate::compact_pose::Contract=serde_json::from_value(sensor.metadata.as_ref().unwrap().value.clone())?;
                    let snapshot=held.snapshot(&sensor.reference,&contract)?;
                    let sample=Frame {observation:snapshot.latest,producer:snapshot.producer,
                        received:snapshot.received,received_hz:snapshot.received_hz};
                    contract.validate_output(&sample.producer)?;
                    ensure!(sample.observation.output==sample.producer.reference(),"Pose producer mismatch");
                    sample.observation.payload.validate()?;
                    let row=catalogs.borrow().get(&sensor.reference.peer_id).cloned().ok_or_else(||anyhow!("Peer catalog unavailable"))?;
                    let (resolved,clock)=crate::product_registry::resolved_contract(&row["registries"],&contract)?;
                    ensure!(sample.producer.spatial_frame.as_ref().is_some_and(|f| f.definition==resolved.to_frame),"Registry frame differs from output");
                    let resolved_pose=json!({"contract":resolved,"clock_definition":clock,"references":contract});
                    sample.observation.payload.validate()?;
                    ensure!(sample.observation.output==sample.producer.reference(),"Pose output changed");
                    let home_pose=if let Some(target)=&request.home_frame {
                        let view=local_scenes.snapshots.keys().filter_map(|id|local_scenes.view(id).ok()).find(|v|v["frame"]==*target).ok_or_else(||anyhow!("Unknown Home scene frame"))?;
                        let (alignment,after)=crate::frame_alignment::resolve(held,sensor,&row,&contract,&peer.peer_id().to_string(),&view,sample.observation.timestamp_ns)?;
                        ensure!(sample.observation.timestamp_ns>=after,"Pose predates alignment");
                        let tracking=crate::frame_alignment::map_tracking(&sample.observation.payload,&alignment,sample.received.elapsed())?;
                        Some(json!({"frame":target,"tracking":tracking,"alignment_timestamp_ns":after.to_string()}))
                    }else{None};
                    let age=sample.received.elapsed();
                    if request.follow {
                        ensure!(age<Duration::from_secs(2),"Sensor pose is stale");
                        let home=home_pose.as_ref().ok_or_else(||anyhow!("Select a Home frame"))?;
                        ensure!(home["tracking"]["status"]=="valid","Sensor tracking is not valid");
                        let pose=serde_json::from_value(home["tracking"]["pose"].clone())?;
                        let source=serde_json::from_value(resolved_pose["contract"]["from_frame"].clone())?;
                        let mut view=crate::view_camera::frame_view(&pose,&source)?;
                        view["frame"]=home["frame"].clone();view["sensor_id"]=json!(sensor.id);
                        view["source_frame"]=json!(contract.from_frame);view["manifest_hash"]=json!(sensor.reference.manifest_hash);
                        view["age_ms"]=json!(age.as_millis() as u64);view["clock"]=json!(contract.clock);
                        view["sequence"]=json!(sample.observation.sequence.to_string());view["capture_timestamp_ns"]=json!(sample.observation.timestamp_ns.to_string());
                        return Ok(view);
                    }
                    let history=snapshot.samples.iter().skip(snapshot.samples.len().saturating_sub(300)).map(|observation| {
                        ensure!(observation.output==sample.producer.reference(),"Pose producer mismatch");
                        observation.payload.validate()?;
                        Ok(json!({"observation":observation,
                            "sequence":observation.sequence.to_string(),
                            "timestamp_ns":observation.timestamp_ns.to_string()}))
                    }).collect::<Result<Vec<Value>>>()?;
                    return Ok(json!({"product":sensor.reference,"stream":if contract.lifetime==crate::compact_pose::Lifetime::Held {"held"} else if age<Duration::from_secs(2) {"live"} else {"stale"},
                        "home_pose":home_pose,
                        "history":history,"history_limit":300,
                        "retention":crate::observation_history::coverage(snapshot.samples.iter().map(|o|o.timestamp_ns)),
                        "capture_utc":held.1.describe(&contract.clock,sample.observation.timestamp_ns),
                        "age_ms":age.as_millis() as u64,"received_hz":if age<Duration::from_secs(2) {sample.received_hz} else {0.0},
                        "resolved":resolved_pose,"observation":sample.observation,
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
                return read_scene_retained(&client, scene, &mut fetched_history).await;
            }
            let Some(id) = &request.sensor else {
                return Ok(
                    json!({"sensors":sensors.iter().map(|s|json!({"id":s.id,"label":s.reference.product_id,"peer_id":s.reference.peer_id,"ar_available":s.ar_supported})).collect::<Vec<_>>()}),
                );
            };
            let sensor = sensors
                .iter()
                .find(|s| &s.id == id)
                .ok_or_else(|| anyhow!("Sensor unavailable; waiting for its peer"))?;
            if request.ar {
                ensure!(
                    sensor.ar_supported,
                    "Camera registry calibration unresolved or unsupported"
                );
            }
            let decoder=crate::camera_frame::decoder(&sensor.datatype)?;
            ensure!(previews.contains_key(id) || previews.len()<8,"Home camera history limit reached (8 open cameras)");
            camera_history.lock().unwrap().reserve(&sensor.reference)?;
            let preview = previews
                .entry(id.clone())
                .or_insert_with(|| start_preview(client.clone(), sensor.clone(),ObservationStart::LatestExisting,|f:&VideoFrame|f.bytes.len(),decoder));
            preview.used = Instant::now();
            let observations = preview
                .frames
                .borrow()
                .clone()
                .ok_or_else(|| anyhow!("{}", &*preview.health.borrow()))?;
            let o = &observations.observation;
            ensure!(
                observations.received.elapsed() < Duration::from_secs(2),
                "Camera frame is stale · {}", &*preview.health.borrow()
            );
            let f = &o.payload;
            ensure!(o.output==observations.producer.reference(),"Camera output changed");
            crate::camera_frame::validate_output(f,&observations.producer)?;
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
            let row={catalogs.borrow().get(&sensor.reference.peer_id).cloned()};
            let presentation=if let Some((_,value))=preview.presentation.as_ref().filter(|(sequence,_)|*sequence==o.sequence) {value.clone()}else if let Some(row)=row {
                crate::video_presentation::resolve(held,sensor,&row,&observations.producer,o.timestamp_ns).await
            }else{json!({"source":"unavailable","detail":"Peer catalog unavailable"})};
            // A missed lookup (slow Wi-Fi, a pose not yet arrived) keeps the last upright
            // angle this camera had, rather than telling the page to show it sideways.
            let presentation=if presentation["rotation_clockwise_deg"].is_u64() {presentation}
                else if let Some(last)=preview.presentation.as_ref().map(|(_,v)|v).filter(|v|v["rotation_clockwise_deg"].is_u64()) {
                    let mut kept=last.clone();kept["kept_after"]=presentation["detail"].clone();kept
                } else {presentation};
            preview.presentation=Some((o.sequence,presentation.clone()));
            if request.ar || request.follow || (request.pick && sensor.ar_supported) {
                if request.follow {ensure!(request.expected_manifest.as_deref()==Some(sensor.reference.manifest_hash.as_str()),"Camera Product changed; attach again");}
                let deadline = tokio::time::Instant::now() + POSE_WAIT;
                let calibrated = loop {
                let attempt = async {
                    let row = catalogs.borrow().get(&sensor.reference.peer_id).cloned()
                        .ok_or_else(|| anyhow!("Camera peer no longer discovered"))?;
                    crate::registry_ar::render_with_history(&client, sensor, &row, o, &observations.producer,
                        local_scenes, &peer.peer_id().to_string(), request.ar, Some(held)).await
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
                    Ok(mut frame) => {
                        ensure!(observations.received.elapsed()<Duration::from_secs(2),"Camera frame became stale while resolving its transforms");
                        if request.follow {
                            ensure!(Some(frame["picking"]["frame"].as_str().unwrap_or(""))==request.home_frame.as_deref(),"Camera is aligned to another Home scene");
                            ensure!(observations.received.elapsed()<Duration::from_secs(2),"Camera pose is stale");
                            let mut view=frame["viewpoint"].take();view["frame"]=frame["picking"]["frame"].clone();
                            view["source_frame"]=frame["source_frame"].clone();view["sensor_id"]=json!(sensor.id);view["manifest_hash"]=json!(sensor.reference.manifest_hash);
                            view["age_ms"]=json!(observations.received.elapsed().as_millis() as u64);view["clock"]=frame["clock"].clone();
                            view["sequence"]=frame["sequence"].clone();view["capture_timestamp_ns"]=frame["capture_timestamp_ns"].clone();return Ok(view);
                        }
                        image=Some(f.bytes.clone());frame["presentation"]=presentation.clone();return Ok(frame);
                    },
                    Err(e) if request.ar || request.follow => return Err(e),
                    Err(e) => pick_error = Some(e.to_string()),
                }
            }
            image=Some(f.bytes.clone());
            Ok(json!({"presentation":presentation,"pick_error":pick_error,"spatial_frame":observations.producer.spatial_frame,"metadata":sensor.metadata,"sequence":o.sequence.to_string(),"capture_timestamp_ns":o.timestamp_ns.to_string(),"clock":observations.producer.clock,"width":f.width,"height":f.height,"encoding":f.encoding}))
        };
        let result = tokio::select! {_ = stop.cancelled()=>break,r=tokio::time::timeout(Duration::from_secs(8),work)=>r.unwrap_or_else(|_|Err(anyhow!("Sensor peer timed out")))};
        if let Some(preview) = request.sensor.as_ref().and_then(|id| previews.get_mut(id)) {
            preview.used = Instant::now();
        }
        let _ = request.reply.send(result.map(|mut value| {
            if let Some(preview)=request.sensor.as_ref().and_then(|id|previews.get(id)) {
                if let Some(retained)=preview.retained.lock().unwrap().as_ref() {
                    let entries=retained.buffer().snapshot(0,u64::MAX);
                    value["retention"]=crate::observation_history::coverage(entries.iter().map(|o|o.payload.timestamp_ns));
                    if let Some(timestamp)=value["capture_timestamp_ns"].as_str().and_then(|s|s.parse().ok()) {
                        value["capture_utc"]=held.1.describe(&retained.producer.clock,timestamp);
                    }
                }
            }
            Reply {value,image}
        }));
    }
    captures.abort_all();
    while captures.join_next().await.is_some() {}
}

#[cfg(test)]
mod preview_timing_tests {
    use super::*;
    #[test]
    fn reconnect_replay_does_not_refresh_stale_frame() {
        let mut last = None;
        assert!(accept_sequence(&mut last, 0));
        assert!(!accept_sequence(&mut last, 0));
        assert!(accept_sequence(&mut last, 23)); // retention gap is allowed
        assert!(!accept_sequence(&mut last, 22));
        assert!(!accept_sequence(&mut last, 23));
        assert!(accept_sequence(&mut last, 24));
    }
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
