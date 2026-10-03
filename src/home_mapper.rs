//! Home-owned mapping from authenticated raw Products. No runner scene input.
use crate::{
    collaborative_mapper::{CollaborativeMap, LocalizedScan},
    local_mapper::{LocalMap, SensorEvent},
    network::HomeConnection,
    scene_mirror::RemoteScene,
};
use anyhow::{Result, anyhow, bail, ensure};
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ObservationStart, RemoteObservationEvent,
    RemoteProductSubscription,
};
use auki_components::*;
use auki_odometry::PoseUpdate;
use auki_sdk::AukiPeer;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        mpsc::{SyncSender, sync_channel},
    },
    time::{Duration, Instant},
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Inbox {
    poses: VecDeque<Observation<PoseUpdate>>,
    camera: Option<Observation<SensorEvent>>,
    lidar: Option<Observation<SensorEvent>>,
}
fn size<T: serde::Serialize>(v: &T) -> usize {
    serde_json::to_vec(v).map_or(usize::MAX, |b| b.len())
}
async fn subscribe<
    T: ContractType + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
>(
    client: &ComponentProtocolClient,
    expected: auki_sdk::PeerId,
    route: auki_sdk::Multiaddr,
    reference: ProductReference,
    history: usize,
) -> Result<RemoteProductSubscription<T>> {
    Ok(client
        .subscribe_product_exact(
            expected,
            route,
            reference,
            if history > 3 {
                ObservationStart::FromSequence { sequence: 0 }
            } else {
                ObservationStart::LatestExisting
            },
            BufferLimits {
                max_entries: Some(history),
                max_bytes: Some(16 * 1024 * 1024),
                target_duration: None,
            },
            size::<T>,
        )
        .await?)
}
enum FusionEvent {
    Scan(Box<LocalizedScan>),
    Progress {
        peer: String,
        metadata: serde_json::Value,
        portals: auki_scenegraph::MapSnapshot,
    },
    Status {
        peer: String,
        status: String,
        detail: String,
    },
}
#[allow(clippy::too_many_arguments)]
pub async fn monitor(
    peer: &AukiPeer,
    configs: &[HomeConnection],
    canonical: &auki_scenegraph::MapSnapshot,
    store: Arc<Mutex<crate::model::Store>>,
    runner_names: std::collections::BTreeMap<String, String>,
    state: &watch::Sender<RemoteScene>,
    ar_frames: &crate::ar_preview::Frames,
    phone_depth: crate::mobile_depth::SharedInbox,
    stop: &CancellationToken,
) {
    let (send, receive) = sync_channel::<FusionEvent>(64);
    let mapping_stop = stop.child_token();
    let task_stop = mapping_stop.clone();
    let task_state = state.clone();
    let owner = peer.peer_id().to_string();
    let reference = canonical.clone();
    let ids = configs
        .iter()
        .map(|c| c.peer_id.clone())
        .collect::<Vec<_>>();
    let writer = tokio::task::spawn_blocking(move || -> Result<()> {
        let session = uuid::Uuid::new_v4().to_string();
        let mut map = CollaborativeMap::new(&owner, &session, &reference)?;
        for peer in ids {
            map.peer_status(&peer, "connecting", "Waiting for sensor Products");
        }
        let checkpoint = std::env::var_os("SPATIAL_HOME_MAP_DATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "data/home-observations.scene.json".into());
        if checkpoint.exists() {
            std::fs::rename(
                &checkpoint,
                checkpoint.with_extension(format!("{session}.archived.json")),
            )?;
        }
        let mut last_publish = Instant::now();
        let mut published = None;
        let mut robot_labels = crate::notes::RobotLabels::default();
        let mut last_labels = Vec::new();
        let mut last_notes = crate::notes::Notes::empty(crate::model::FRAME);
        while !task_stop.is_cancelled() {
            let phone_frames = phone_depth
                .lock()
                .map_err(|_| anyhow!("phone depth unavailable"))?
                .take();
            for frame in phone_frames {
                let peer = frame.source.peer_id.clone();
                if let Err(error) = map.integrate_phone(frame) {
                    map.peer_status(&peer, "rejected", &error.to_string());
                }
            }
            match receive.recv_timeout(Duration::from_millis(25)) {
                Ok(FusionEvent::Scan(scan)) => {
                    let peer = scan.binding.sensor_product.peer_id.clone();
                    if let Err(e) = map.integrate(*scan) {
                        map.peer_status(&peer, "rejected", &format!("{e:#}"));
                    }
                }
                Ok(FusionEvent::Progress {
                    peer,
                    metadata,
                    portals,
                }) => {
                    if let Err(e) = map.peer_progress(&peer, metadata, &portals) {
                        map.peer_status(&peer, "rejected", &format!("{e:#}"));
                    }
                }
                Ok(FusionEvent::Status {
                    peer,
                    status,
                    detail,
                }) => map.peer_status(&peer, &status, &detail),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if last_publish.elapsed() >= Duration::from_millis(500) {
                let notes = crate::notes::from_registries(
                    &store
                        .lock()
                        .map_err(|_| anyhow!("store unavailable"))?
                        .built
                        .registries,
                    crate::model::FRAME,
                )?;
                if notes != last_notes {
                    map.revision += 1;
                    last_notes = notes;
                }
                if let Some(mut scene) = map.scene()? {
                    let labels = robot_labels.update(
                        &scene.registries["local_mapping"]["runners"],
                        &runner_names,
                        &scene.voxels.definition.frame,
                        Instant::now(),
                    )?;
                    if labels != last_labels {
                        map.revision += 1;
                        last_labels = labels;
                    }
                    if published == Some(map.revision) {
                        last_publish = Instant::now();
                        continue;
                    }
                    scene.revision = map.revision;
                    crate::notes::attach_home(&mut scene, &last_notes)?;
                    let mut notes = crate::notes::from_registries(&scene.registries, &scene.frame)?;
                    notes.items.extend(last_labels.clone());
                    notes.validate(&scene.frame)?;
                    scene.registries["text_notes"] = serde_json::json!(notes);
                    let tmp = checkpoint.with_extension("tmp");
                    std::fs::write(&tmp, serde_json::to_vec(&scene)?)?;
                    std::fs::rename(tmp, &checkpoint)?;
                    task_state.send_replace(RemoteScene {
                        scene: Some(scene),
                        status: "connected".into(),
                        detail: map.detail(),
                        received: Some(Instant::now()),
                    });
                } else {
                    task_state.send_modify(|s| {
                        s.status = "waiting_for_portal".into();
                        s.detail = map.detail();
                    });
                }
                published = Some(map.revision);
                last_publish = Instant::now();
            }
        }
        Ok(())
    });
    // Poll borrowed connection futures concurrently without creating more peers.
    let mut readers: Vec<_> = configs
        .iter()
        .map(|c| {
            Box::pin(monitor_runner(
                peer,
                c,
                canonical,
                send.clone(),
                ar_frames.clone(),
                &mapping_stop,
            ))
        })
        .collect();
    let connections = async {
        std::future::poll_fn(|cx| {
            readers
                .retain_mut(|future| std::future::Future::poll(future.as_mut(), cx).is_pending());
            if readers.is_empty() {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        })
        .await;
    };
    let fusion = async {
        let result = writer.await;
        if !matches!(result, Ok(Ok(()))) {
            state.send_modify(|s| {
                s.status = "failed".into();
                s.detail = format!("Home fusion stopped: {result:?}");
            });
        }
        mapping_stop.cancel();
    };
    tokio::join!(connections, fusion);
}
async fn monitor_runner(
    peer: &AukiPeer,
    config: &HomeConnection,
    canonical: &auki_scenegraph::MapSnapshot,
    send: SyncSender<FusionEvent>,
    ar_frames: crate::ar_preview::Frames,
    stop: &CancellationToken,
) {
    loop {
        let status = |state: &str, detail: &str| {
            let _ = send.try_send(FusionEvent::Status {
                peer: config.peer_id.clone(),
                status: state.into(),
                detail: detail.into(),
            });
        };
        status(
            "connecting",
            "Subscribing to raw odometry, camera/depth and LiDAR",
        );
        let result = run(
            peer,
            config,
            canonical,
            send.clone(),
            ar_frames.clone(),
            stop,
        )
        .await;
        if stop.is_cancelled() {
            break;
        }
        if let Err(e) = result {
            status("disconnected", &format!("{e:#}"));
        }
        tokio::select! {_=stop.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(3))=>{}}
    }
}
async fn run(
    peer: &AukiPeer,
    config: &HomeConnection,
    canonical: &auki_scenegraph::MapSnapshot,
    send: SyncSender<FusionEvent>,
    ar_frames: crate::ar_preview::Frames,
    stop: &CancellationToken,
) -> Result<()> {
    let expected = config.validate(peer)?;
    let client = ComponentProtocolClient::new(peer.protocols());
    let mut selected = None;
    for address in &config.routes {
        let route = address.parse()?;
        let response = tokio::select! { _=stop.cancelled()=>return Ok(()), r=tokio::time::timeout(Duration::from_secs(10),client.catalog_exact(expected,route,None))=>r };
        if let Ok(Ok(CatalogResponse::Snapshot { snapshot })) = response {
            selected = Some((address.parse::<auki_sdk::Multiaddr>()?, snapshot));
            break;
        }
    }
    let (route, catalog) = selected.ok_or_else(|| anyhow!("Runner sensor catalog unavailable"))?;
    let reference = |id: &str| -> Result<_> {
        let entry = catalog
            .products
            .iter()
            .find(|p| p.manifest.product_id == id)
            .ok_or_else(|| anyhow!("Missing raw Product {id}"))?;
        ensure!(
            entry.manifest.peer_id == config.peer_id,
            "Unexpected sensor owner/schema"
        );
        Ok(ProductReference {
            peer_id: config.peer_id.clone(),
            product_id: id.into(),
            manifest_hash: entry.manifest_hash.clone(),
        })
    };
    let camera_ref = reference("local-camera-observations")?;
    let lidar_ref = reference("local-lidar-history")?;
    let odom_ref = reference("local-odometry-history")?;
    let mut odom = subscribe::<PoseUpdate>(&client, expected, route.clone(), odom_ref, 600).await?;
    let mut camera =
        subscribe::<SensorEvent>(&client, expected, route.clone(), camera_ref.clone(), 3).await?;
    let mut lidar =
        subscribe::<SensorEvent>(&client, expected, route, lidar_ref.clone(), 3).await?;
    ensure!(
        odom.product().producer.payload.schema() == auki_odometry::POSE_SCHEMA
            && camera.product().producer.payload.schema() == SensorEvent::DATATYPE
            && lidar.product().producer.payload.schema() == SensorEvent::DATATYPE,
        "Unexpected raw sensor schema"
    );
    let clock = odom.product().producer.clock.clone();
    ensure!(
        clock.peer_id == config.peer_id
            && camera.product().producer.clock == clock
            && lidar.product().producer.clock == clock,
        "Sensor capture clocks do not agree"
    );
    let camera_frame = camera
        .product()
        .producer
        .spatial_frame
        .as_ref()
        .ok_or_else(|| anyhow!("raw camera optical frame is undeclared"))?
        .definition
        .clone();
    let inbox = Arc::new(Mutex::new(Inbox::default()));
    let worker_stop = stop.child_token();
    let owner = peer.peer_id().to_string();
    let task_inbox = inbox.clone();
    let task_stop = worker_stop.clone();
    let reference = canonical.clone();
    let worker = tokio::task::spawn_blocking(move || -> Result<()> {
        let session = uuid::Uuid::new_v4().to_string();
        let runtime = ComponentRuntime::new(owner.clone());
        let mut mapper: Option<LocalMap> = None;
        let mut last_publish = Instant::now();
        let mut last_pose_time = 0;
        while !task_stop.is_cancelled() {
            let batch = {
                let mut q = task_inbox
                    .lock()
                    .map_err(|_| anyhow!("mapper inbox unavailable"))?;
                std::mem::take(&mut *q)
            };
            for o in batch.poses {
                o.payload.validate()?;
                ensure!(
                    o.payload.contract.clock == clock
                        && o.payload.contract.from_frame.peer_id == clock.peer_id
                        && o.payload.contract.to_frame.peer_id == clock.peer_id,
                    "Odometry provenance mismatch"
                );
                if mapper.is_none() {
                    mapper = Some(LocalMap::from_runner_with_camera(
                        &runtime,
                        &owner,
                        &session,
                        o.payload.contract.clone(),
                        camera_ref.clone(),
                        lidar_ref.clone(),
                        None,
                        camera_frame.clone(),
                    )?);
                    let map = mapper.as_mut().unwrap();
                    map.calibrate_against(&reference)?;
                    map.emit_localized_scans();
                    ar_frames
                        .lock()
                        .map_err(|_| anyhow!("AR cache unavailable"))?
                        .poses
                        .insert(clock.peer_id.clone(), map.ar_pose_history());
                }
                // A reconnect creates a fresh map; duplicates in a replay are ignored.
                if o.timestamp_ns > last_pose_time {
                    mapper
                        .as_mut()
                        .unwrap()
                        .accept_pose(o.timestamp_ns, &o.payload)?;
                    last_pose_time = o.timestamp_ns;
                }
            }
            if let Some(map) = mapper.as_mut() {
                for o in [batch.camera, batch.lidar].into_iter().flatten() {
                    map.accept_remote(o)?;
                }
                if let Some(origin) = &map.origin {
                    ar_frames
                        .lock()
                        .map_err(|_| anyhow!("AR cache unavailable"))?
                        .record_alignment(&clock.peer_id, origin);
                }
                if let Some(frame) = map.ar_frame.take() {
                    ar_frames
                        .lock()
                        .map_err(|_| anyhow!("AR frame cache unavailable"))?
                        .frames
                        .insert(clock.peer_id.clone(), frame);
                }
                for scan in map.take_localized_scans() {
                    // Backpressure is bounded. Preserve the newest measurements
                    // through the per-source ingress mailbox, not an ever-growing queue.
                    match send.try_send(FusionEvent::Scan(Box::new(scan))) {
                        Ok(()) | Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                            bail!("Home fusion stopped")
                        }
                    }
                }
                if last_publish.elapsed() >= Duration::from_millis(500) {
                    let _ = send.try_send(FusionEvent::Progress {
                        peer: clock.peer_id.clone(),
                        metadata: map.metadata(),
                        portals: map.portals()?,
                    });
                    last_publish = Instant::now();
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    });
    let work=async {
        let mut last_pose=Instant::now();
        let mut health=tokio::time::interval(Duration::from_secs(1));
        loop {
            ensure!(!worker.is_finished(),"Home mapping worker stopped");
            tokio::select! {
                _=stop.cancelled()=>break,
                _=health.tick()=>{ensure!(last_pose.elapsed()<Duration::from_secs(15),"Raw odometry stream is stale");},
                event=odom.next()=>match event? {
                    Some(RemoteObservationEvent::Observation(o))=>{last_pose=Instant::now();let mut q=inbox.lock().unwrap(); if q.poses.len()>=600 {bail!("Home odometry consumer fell behind");} q.poses.push_back(o);},
                    Some(RemoteObservationEvent::Gap(_))=>{},
                    _=>bail!("Odometry stream ended"),
                },
                event=camera.next()=>match event? {
                    Some(RemoteObservationEvent::Observation(o))=>{ensure!(matches!(*o.payload,SensorEvent::Camera{..}),"unexpected camera event");inbox.lock().unwrap().camera=Some(o);},
                    Some(RemoteObservationEvent::Gap(_))=>{}, _=>bail!("Camera stream ended"),
                },
                event=lidar.next()=>match event? {
                    Some(RemoteObservationEvent::Observation(o))=>{ensure!(matches!(*o.payload,SensorEvent::Lidar{..}),"unexpected LiDAR event");inbox.lock().unwrap().lidar=Some(o);},
                    Some(RemoteObservationEvent::Gap(_))=>{}, _=>bail!("LiDAR stream ended"),
                }
            }
        }
        Ok(())
    }.await;
    worker_stop.cancel();
    let worker_result = worker.await?;
    let _ = tokio::join!(odom.close(), camera.close(), lidar.close());
    worker_result.and(work)
}
