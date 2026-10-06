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
    pub reply: oneshot::Sender<Result<Value>>,
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
    id: String,
    reference: ProductReference,
    route: Option<Multiaddr>,
    peer: PeerId,
    metadata: Option<auki_components::CatalogProductMetadata>,
    pose: Option<ProductReference>,
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

struct Frame {
    observation: Observation<VideoFrame>,
    producer: OutputManifest,
    received: Instant,
}
struct Preview {
    reference: ProductReference,
    used: Instant,
    frames: watch::Receiver<Option<Arc<Frame>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Preview {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn start_preview(client: ComponentProtocolClient, sensor: Sensor) -> Preview {
    let (send, frames) = watch::channel(None);
    let reference = sensor.reference.clone();
    let task = tokio::spawn(async move {
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
                        ObservationStart::LatestExisting,
                        limits,
                        |f: &VideoFrame| f.bytes.len(),
                    )
                    .await
            } else {
                client
                    .subscribe_product(
                        sensor.peer,
                        sensor.reference,
                        ObservationStart::LatestExisting,
                        limits,
                        |f: &VideoFrame| f.bytes.len(),
                    )
                    .await
            }
        };
        let Ok(Ok(mut subscription)) = tokio::time::timeout(Duration::from_secs(5), connect).await
        else {
            return;
        };
        let producer = subscription.product().producer.clone();
        loop {
            match subscription.next().await {
                Ok(Some(RemoteObservationEvent::Observation(observation))) => {
                    send.send_replace(Some(Arc::new(Frame {
                        observation,
                        producer: producer.clone(),
                        received: Instant::now(),
                    })));
                }
                Ok(Some(RemoteObservationEvent::Gap(_))) => continue,
                _ => break,
            }
        }
    });
    Preview {
        reference,
        used: Instant::now(),
        frames,
        task,
    }
}
pub async fn serve(
    peer: &AukiPeer,
    catalogs: watch::Receiver<crate::device_catalogs::Catalogs>,
    mut requests: mpsc::Receiver<Request>,
    home_live: tokio::sync::watch::Receiver<crate::scene_mirror::RemoteScene>,
    ar_frames: &crate::ar_preview::Frames,
    stop: &tokio_util::sync::CancellationToken,
) {
    let client = ComponentProtocolClient::new(peer.protocols());
    let mut previews = BTreeMap::<String, Preview>::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        let sensors = camera_sensors(&catalogs.borrow());
        previews.retain(|id, p| {
            p.used.elapsed() < Duration::from_secs(2)
                && !p.task.is_finished()
                && sensors
                    .iter()
                    .any(|s| s.id == *id && s.reference == p.reference)
        });
        let request = tokio::select! {_ = stop.cancelled()=>break, _ = tick.tick()=>continue, r=requests.recv()=>match r {Some(r)=>r,None=>break}};
        let sensors = camera_sensors(&catalogs.borrow());
        let work = async {
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
                .or_insert_with(|| start_preview(client.clone(), sensor.clone()));
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
                let calibrated = async {
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
                    let live = home_live.borrow();
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
                .await;
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
        let _ = request.reply.send(result);
    }
}
