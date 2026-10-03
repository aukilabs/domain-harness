//! Read-only previews from camera Products owned by configured peers.
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ObservationRequest, ObservationSelection,
};
use auki_components::{ContractType, PayloadContract, ProductReference, VideoFrame};
use auki_sdk::{AukiPeer, Multiaddr, PeerId};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
pub struct Request {
    pub sensor: Option<String>,
    pub ar: bool,
    pub pick: bool,
    pub reply: oneshot::Sender<Result<Value>>,
}
struct Sensor {
    id: String,
    reference: ProductReference,
    route: Multiaddr,
    peer: PeerId,
    metadata: Option<auki_components::CatalogProductMetadata>,
    pose: Option<ProductReference>,
}
pub async fn serve(
    peer: &AukiPeer,
    targets: Vec<(String, Vec<String>)>,
    mut requests: mpsc::Receiver<Request>,
    home_live: tokio::sync::watch::Receiver<crate::scene_mirror::RemoteScene>,
    ar_frames: &crate::ar_preview::Frames,
    stop: &tokio_util::sync::CancellationToken,
) {
    let client = ComponentProtocolClient::new(peer.protocols());
    let mut sensors = Vec::<Sensor>::new();
    let mut last_catalog = None::<std::time::Instant>;
    loop {
        let request = tokio::select! {_ = stop.cancelled()=>break, r=requests.recv()=>match r {Some(r)=>r,None=>break}};
        let work = async {
            if request.sensor.is_none()
                && last_catalog.is_none_or(|t| t.elapsed() > Duration::from_secs(10))
            {
                let mut found = Vec::new();
                for (owner, configured) in &targets {
                    let expected: PeerId = owner.parse()?;
                    let mut routes: Vec<Multiaddr> =
                        configured.iter().filter_map(|s| s.parse().ok()).collect();
                    if routes.is_empty()
                        && let Ok(Ok(peers)) =
                            tokio::time::timeout(Duration::from_secs(2), peer.discover()).await
                    {
                        for p in peers {
                            if p.peer_id() == expected {
                                routes.extend_from_slice(p.routes());
                            }
                        }
                    }
                    for route in routes {
                        if let Ok(Ok(CatalogResponse::Snapshot { snapshot })) =
                            tokio::time::timeout(
                                Duration::from_secs(2),
                                client.catalog_exact(expected, route.clone(), None),
                            )
                            .await
                        {
                            for p in &snapshot.products {
                                if p.manifest.peer_id != *owner {
                                    continue;
                                }
                                // Only typed image Products are eligible, never arbitrary routes from the browser.
                                let Some(_camera) = snapshot.components.iter().flat_map(|c|c.current_outputs.values()).find(|o| o.manifest.reference()==p.manifest.producer && matches!(&o.manifest.payload, PayloadContract::Camera(c) if c.datatype==VideoFrame::DATATYPE)) else {continue;};
                                let pose_id = p
                                    .metadata
                                    .as_ref()
                                    .and_then(|m| m.value["pose_product_id"].as_str());
                                let pose = snapshot.products.iter().find(|g| {
                                    g.manifest.peer_id == *owner && Some(g.manifest.product_id.as_str()) == pose_id
                                    && snapshot.components.iter().flat_map(|c|c.current_outputs.values()).any(|o|
                                        o.manifest.reference()==g.manifest.producer && matches!(&o.manifest.payload,
                                            PayloadContract::Structured(c) if c.datatype==auki_odometry::PoseUpdate::DATATYPE))
                                }).map(|g| ProductReference { peer_id: owner.clone(), product_id:g.manifest.product_id.clone(), manifest_hash:g.manifest_hash.clone() });
                                let id = format!("{owner}/{}", p.manifest.product_id);
                                found.push(Sensor {
                                    id,
                                    reference: ProductReference {
                                        peer_id: owner.clone(),
                                        product_id: p.manifest.product_id.clone(),
                                        manifest_hash: p.manifest_hash.clone(),
                                    },
                                    route: route.clone(),
                                    peer: expected,
                                    metadata: p.metadata.clone(),
                                    pose,
                                });
                            }
                            break;
                        }
                    }
                }
                sensors = found;
                last_catalog = Some(std::time::Instant::now());
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
            let observations = client
                .observations_exact::<VideoFrame>(
                    sensor.peer,
                    sensor.route.clone(),
                    ObservationRequest {
                        product: sensor.reference.clone(),
                        selection: ObservationSelection::LatestExisting,
                    },
                )
                .await?;
            ensure!(
                observations.product.reference() == sensor.reference
                    && observations.producer.peer_id == sensor.reference.peer_id,
                "Camera owner changed"
            );
            ensure!(
                observations.gap.is_none() && observations.end.is_none(),
                "Camera stream ended"
            );
            let o = observations
                .observations
                .last()
                .ok_or_else(|| anyhow!("Waiting for camera frames"))?;
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
                    let pose = client
                        .observations_exact::<auki_odometry::PoseUpdate>(
                            sensor.peer,
                            sensor.route.clone(),
                            ObservationRequest {
                                product: reference.clone(),
                                selection: ObservationSelection::TimeRange {
                                    request: auki_components::TimeRangeRequest {
                                        clock: observations.producer.clock.clone(),
                                        start_ns: o.timestamp_ns,
                                        end_ns: o.timestamp_ns,
                                    },
                                },
                            },
                        )
                        .await?;
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
