//! One peer/runtime; acquisition, sensors, mapping and disk work run independently.
use crate::{
    fresh_ingress::FreshIngress,
    local_mapper::{LocalMap, SensorEvent},
    network::{CanonicalScene, HomeConnection, ScenePublisher},
    peer::PeerSettings,
};
use anyhow::{Result, anyhow, ensure};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, BufReader};
type Inbox = Arc<Mutex<FreshIngress>>;
#[path = "robot_live.rs"]
mod live;
#[derive(serde::Serialize)]
struct DiskState {
    scene: Option<CanonicalScene>,
    catalog: auki_components::CatalogSnapshot,
    status: serde_json::Value,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hardware {
    Galbot,
    Bracketbot,
}

pub async fn run(hardware: Hardware) -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4,
        "Usage: robot-runner PEER.json SCENE.json PROFILE.json"
    );
    let path = PathBuf::from(&args[2]);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.try_lock()?;
    let running = PeerSettings::load(Path::new(&args[1]))?.start().await?;
    let session = uuid::Uuid::new_v4().to_string();
    let family = match hardware {
        Hardware::Galbot => crate::local_mapper::FAMILY,
        Hardware::Bracketbot => crate::local_mapper::BRACKETBOT_FAMILY,
    };
    let frame = format!("{family}:{session}:zup");
    let mut publisher = ScenePublisher::mount_in_frame(&running.peer, &frame)?;
    let owner = running.peer.peer_id().to_string();
    let mut local = match hardware {
        Hardware::Galbot => LocalMap::new(&publisher.runtime, &owner, &session)?,
        Hardware::Bracketbot => LocalMap::new_for_robot(
            &publisher.runtime,
            &owner,
            &session,
            family,
            crate::rgbd::BRACKETBOT_FRAME,
            serde_json::from_str(include_str!(
                "../config/humanoid-hub-conference/portals.json"
            ))?,
        )?,
    };
    if let Some(path) = std::env::var_os("SPATIAL_PORTAL_SIZES") {
        local.set_portal_sizes(crate::portal_sizes::PortalSizes::load(path)?)?;
    }
    local.export_sources(&publisher.endpoint)?;
    local.copy_native_scans();
    let contribution_clock = crate::model::clock_entry(&owner, &session);
    let contribution_owner = owner.clone();
    let (contribution_send, mut contribution_receive) = tokio::sync::mpsc::channel::<crate::scene_contribution::Contribution>(2);
    let mut connection = HomeConnection::current(&running.peer)?;
    connection.frame = if hardware == Hardware::Bracketbot {
        family.into()
    } else {
        frame.clone()
    };
    save(Path::new(&args[3]), &connection)?;
    if path.exists() {
        std::fs::rename(
            &path,
            path.with_extension(format!("previous-{session}.json")),
        )?;
    }
    let script = std::env::var_os("SPATIAL_LOCAL_CAPTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| match hardware {
            Hardware::Galbot => "scripts/capture-runner.py".into(),
            Hardware::Bracketbot => "scripts/capture-bracketbot.py".into(),
        });
    let python = std::env::var_os("SPATIAL_CAPTURE_PYTHON").unwrap_or_else(|| match hardware {
        Hardware::Galbot => "python3.8".into(),
        Hardware::Bracketbot => "python3".into(),
    });
    let context = serde_json::to_string(&serde_json::json!({
        "clock":crate::model::clock_ref(&crate::model::clock_entry(&owner,&session)),
        "session_id":session,
        "control_state_path":path.with_file_name("robot-control.json"),
        "camera_frame":crate::rgbd::BRACKETBOT_FRAME,
        "camera_frame_definition":auki_registry::FrameRegistryEntry::ros_optical(&owner,crate::rgbd::BRACKETBOT_FRAME),
        "base_frame_definition":auki_registry::FrameRegistryEntry::in_convention(&owner,"robot.base",auki_registry::CoordinateConvention::ZUpRightForward),
    }))?;
    let mut command = tokio::process::Command::new(python);
    if hardware == Hardware::Bracketbot {
        command.arg("scripts/bracketbot-python.py");
    } else {
        command.env("PYTHONPATH","/data/galbot/lib:/data/galbot/lib/python3.8.10:/data/galbot/lib/python3/site-packages")
            .env("LD_LIBRARY_PATH","/data/galbot/lib");
    }
    let mut child = command
        .arg(script)
        .env("GALBOT_SENSOR_STATE", path.parent().unwrap())
        .env("GALBOT_CAPTURE_CONTEXT", &context)
        .env("SPATIAL_CAPTURE_CONTEXT", &context)
        .env("OPENBLAS_NUM_THREADS", "1")
        .env("OMP_NUM_THREADS", "1")
        .env_remove("AUKI_ACCESS_KEY")
        .env_remove("AUKI_APP_SECRET")
        .env_remove("AUKI_EMAIL")
        .env_remove("AUKI_PASSWORD")
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let sensors: Inbox = Default::default();
    let mapping: Inbox = Default::default();
    let live = Arc::new(Mutex::new(live::LiveState::new(hardware == Hardware::Bracketbot)));
    let stop = Arc::new(AtomicBool::new(false));
    let disk = Arc::new(Mutex::new(None::<DiskState>));
    let sensor_status = Arc::new(Mutex::new(serde_json::json!({})));
    let home_status = Arc::new(Mutex::new(serde_json::json!({"status":"connecting"})));
    let (home_send, home_receive) = std::sync::mpsc::sync_channel::<(String, auki_scenegraph::MapSnapshot)>(1);
    let (origin_send,mut origin_receive)=tokio::sync::mpsc::channel::<crate::scene_origin::Observation>(1);
    let reader = {
        let live = live.clone();
        let sensors = sensors.clone();
        let mapping = mapping.clone();
        tokio::spawn(async move {
            while let Some(line) = lines.next_line().await? {
                if let Some(raw) = line.strip_prefix("SENSOR ") {
                    ensure!(raw.len() < 16 * 1024 * 1024, "sensor packet exceeds limit");
                    let value: serde_json::Value = serde_json::from_str(raw)?;
                    if matches!(value["kind"].as_str(), Some("odometry" | "tracking_lost" | "map_correction" | "trajectory_correction" | "correction_pending")) {
                        let event: SensorEvent = serde_json::from_value(value.clone())?;
                        live.lock().unwrap().accept(&event);
                    }
                    let q = if matches!(
                        value["kind"].as_str(),
                        Some("sensor_sample" | "register_sensor")
                    ) {
                        &sensors
                    } else {
                        &mapping
                    };
                    q.lock().unwrap().push(value)?;
                }
            }
            Err::<(), _>(anyhow!("sensor capture ended"))
        })
    };
    let sensor_worker = {
        let stop = stop.clone();
        let runtime = publisher.runtime.clone();
        let endpoint = publisher.endpoint.clone();
        let status = sensor_status.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let detector_budget = Arc::new(crate::portal_camera_detector::DetectorBudget::default());
            let mut mounted =
                std::collections::BTreeMap::<String, crate::galbot_sensors::Sensor>::new();
            let work = (|| -> Result<()> {
                while !stop.load(Ordering::Relaxed) {
                    let batch = sensors.lock().unwrap().drain();
                    for value in batch {
                        if value["kind"] == "register_sensor" {
                            let spec: crate::galbot_sensors::SensorSpec =
                                serde_json::from_value(value["spec"].clone())?;
                            ensure!(!mounted.contains_key(&spec.id), "duplicate sensor");
                            let mut sensor =
                                crate::galbot_sensors::Sensor::mount(&runtime, &endpoint, spec)
                                    .map_err(|e| anyhow!(e))?;
                            if matches!(hardware, Hardware::Bracketbot)
                                && matches!(
                                    sensor.spec.id.as_str(),
                                    "camera.left"
                                        | "camera.right"
                                        | "camera.head"
                                        | "camera.head.right"
                                )
                            {
                                sensor.portal_detector = Some(
                                    crate::portal_camera_detector::PortalCameraDetector::mount(
                                        &runtime,
                                        &endpoint,
                                        &sensor.spec,
                                        detector_budget.clone(),
                                    )?,
                                );
                            }
                            mounted.insert(sensor.spec.id.clone(), sensor);
                        } else {
                            use base64::Engine;
                            let id = value["sensor_id"]
                                .as_str()
                                .ok_or_else(|| anyhow!("sensor ID missing"))?;
                            let sensor = mounted
                                .get_mut(id)
                                .ok_or_else(|| anyhow!("unregistered sensor {id}"))?;
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(value["bytes"].as_str().unwrap_or(""))?;
                            if let Err(e) = sensor.publish(
                                value["timestamp_ns"].as_u64().unwrap_or(0),
                                value["data"].as_str().unwrap_or(""),
                                bytes,
                            ) {
                                eprintln!("Sensor {id}: {e}");
                            }
                        }
                    }
                    *status.lock().unwrap() = serde_json::json!({"sources":mounted.values().map(|s|s.status()).collect::<Vec<_>>(),"coalesced_samples":sensors.lock().unwrap().replaced});
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            })();
            for sensor in mounted.values() {
                sensor.end();
            }
            work
        })
    };
    let mapper = {
        let live = live.clone();
        let stop = stop.clone();
        let home_status = home_status.clone();
        let disk = disk.clone();
        tokio::task::spawn_blocking(move || {
            let work = (|| -> Result<()> {
                let mut last_publish = Instant::now() - Duration::from_secs(1);
                let mut last_odom = Instant::now();
                let mut capture_status = String::from("Starting sensor capture");
                let mut pending_home = None;
                let mut origin_after=0;
                let mut last_alignment_attempt = Instant::now() - Duration::from_secs(1);
                while !stop.load(Ordering::Relaxed) {
                    let started = Instant::now();
                    let batch = mapping.lock().unwrap().drain();
                    for value in batch {
                        let event: SensorEvent = serde_json::from_value(value)?;
                        if let SensorEvent::Status { detail } = &event {
                            capture_status.clone_from(detail);
                        }
                        let odom = matches!(event, SensorEvent::Odometry { .. } | SensorEvent::MapCorrection { .. } | SensorEvent::TrajectoryCorrection { .. });
                        match local.accept(event) {
                            Ok(()) => {
                                if odom {
                                    last_odom = Instant::now();
                                }
                            }
                            Err(e) => {
                                local.detail = format!("Observation rejected: {e}");
                                eprintln!("{}", local.detail);
                            }
                        }
                    }
                    if let Ok(home) = home_receive.try_recv() {
                        if home.1.scenegraph.anchors.is_empty() && pending_home.as_ref().is_none_or(|old:&(String,auki_scenegraph::MapSnapshot)|old.1!=home.1) {
                            origin_after=local.latest_pose.as_ref().map_or(0,|(t,_)|*t);
                        }
                        pending_home = Some(home);
                        // Refresh changed Home references as well as retaining
                        // them for re-alignment after a local map correction.
                        local.home_portals = None;
                    }
                    // Fetch references before detection; apply only when a measured
                    // Portal exists. Loss of SLAM must not hide connection failures.
                    if local.home_portals.is_none()
                        && last_alignment_attempt.elapsed() >= Duration::from_secs(1)
                        && !local.portals()?.scenegraph.anchors.is_empty()
                        && let Some((peer_id, portals)) = pending_home.as_ref()
                    {
                        last_alignment_attempt = Instant::now();
                        if portals.scenegraph.anchors.is_empty() {
                            let measured=local.portals()?;
                            let metadata=local.metadata();
                            let first=metadata["local_mapping"]["observed_portals"].as_object().into_iter().flatten()
                                .filter_map(|(id,e)|e["capture_timestamp_ns"].as_u64().filter(|t|*t>origin_after).map(|t|(t,id)))
                                .min_by_key(|(t,_)|*t);
                            if let Some((timestamp,id))=first && let Some(anchor)=measured.scenegraph.anchors.get(id) {
                                let mut pose=anchor.pose_in_map.clone();pose.from_frame_id=crate::mobile_portal::local_frame(id);
                                let _=origin_send.try_send(crate::scene_origin::Observation{expected_scene:portals.clone(),code:id.clone(),side_length_m:anchor.side_length_m,timestamp_ns:timestamp,portal_to_native:pose});
                            }
                        } else if let Err(e) = local.accept_home_portals(portals.clone(), peer_id.clone()) {
                            eprintln!("Home references: {e}");
                        }
                    }
                    if last_odom.elapsed() > Duration::from_secs(3) {
                        if hardware != Hardware::Bracketbot && let Some((timestamp, _)) = &local.latest_pose {
                            local.alignment_after_ns = Some(local.metadata()["local_mapping"]["observed_portals"]
                                .as_object().into_iter().flat_map(|m|m.values())
                                .filter_map(|v|v["capture_timestamp_ns"].as_u64())
                                .max().unwrap_or(*timestamp).max(*timestamp));
                        }
                        local.latest_pose = None;
                    }
                    local.export_camera_preview(&publisher.endpoint)?;
                    let scans = local.take_localized_scans();
                    if let Ok(transform) = crate::frame_transform::FrameTransform::from_local(&local, &contribution_owner) {
                        if transform.validity == crate::frame_transform::Validity::Valid {
                            if let (Some(alignment),Some(evidence)) = (&transform.transform,&transform.evidence) {
                                let measured = local.portals()?;
                                if let Some(anchor) = measured.scenegraph.anchors.get(&evidence.portal) {
                                    for scan in scans {
                                        let o=scan.observation;
                                        if o.timestamp_ns < evidence.capture_timestamp_ns { continue; }
                                        let c=crate::scene_contribution::Contribution {
                                            source:o.source,sequence:o.sequence,clock:contribution_clock.clone(),timestamp_ns:o.timestamp_ns,
                                            sensor_frame:scan.binding.sensor_frame,
                                            native_frame:crate::conventions::registry(&contribution_owner,&local.frame)?,
                                            sensor_to_native:o.pose.sensor_to_map,alignment_timestamp_ns:evidence.capture_timestamp_ns,
                                            portal_id:evidence.portal.clone(),portal_to_native:crate::scene_contribution::portal_measurement(anchor, &evidence.reference_anchor)?,
                                            native_to_scene:alignment.clone(),points:o.hit_points_m,colors:scan.colors,
                                        };
                                        let _=contribution_send.try_send(c);
                                    }
                                }
                            }
                        }
                    }
                    if last_publish.elapsed() >= Duration::from_millis(250) {
                        let scene = publisher.publish_local(&local)?;
                        if let Some(scene) = &scene {
                            live.lock().unwrap().set_basis(scene);
                        }
                        let status = serde_json::json!({"frame":frame,"origin":local.origin,"detail":local.detail,"capture_status":capture_status,"scan_progress":local.scan_progress(),"status":if local.latest_pose.is_some(){"mapping"}else{"waiting_for_odometry"},"mapper_batch_ms":started.elapsed().as_millis(),"coalesced_samples":mapping.lock().unwrap().replaced,"sensors":sensor_status.lock().unwrap().clone(),"home_reference":home_status.lock().unwrap().clone(),"home_reference_applied":local.home_portals.is_some()});
                        *disk.lock().unwrap() = Some(DiskState {
                            scene,
                            catalog: publisher.runtime.catalog().snapshot(),
                            status,
                        });
                        last_publish = Instant::now();
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            })();
            (publisher, work)
        })
    };
    let writer = {
        let stop = stop.clone();
        let disk = disk.clone();
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            loop {
                let snapshot = disk.lock().unwrap().take();
                if let Some(s) = snapshot {
                    if let Some(scene) = &s.scene {
                        save(&path, scene)?;
                    }
                    save(&path.with_extension("catalog.json"), &s.catalog)?;
                    save(&path.with_extension("status.json"), &s.status)?;
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Ok(())
        })
    };
    let home_path = std::env::var_os("SPATIAL_HOME_CONNECTION")
        .map(PathBuf::from)
        .unwrap_or_else(|| path.parent().unwrap().join("home-connection.json"));
    let home_work = async {
        loop {
            let mut refresh_seconds=30;
            let fetch = async {
                let profile: HomeConnection = serde_json::from_slice(&std::fs::read(&home_path)?)?;
                let portals = crate::network::fetch_home_portals(&running.peer, &profile).await?;
                Ok::<_, anyhow::Error>((profile.peer_id, portals))
            };
            match fetch.await {
                Ok((peer_id, portals)) => {
                    if portals.scenegraph.anchors.is_empty(){refresh_seconds=3;}
                    *home_status.lock().unwrap() = serde_json::json!({"status":"received", "peer_id":peer_id,"frame":portals.scenegraph.map.frame.id});
                    let _ = home_send.try_send((peer_id, portals));
                }
                Err(e) => {
                    *home_status.lock().unwrap() =
                        serde_json::json!({"status":"unavailable", "error":e.to_string()});
                    eprintln!("Home reference fetch: {e}");
                }
            }
            tokio::time::sleep(Duration::from_secs(refresh_seconds)).await;
        }
    };
    let origin_work=async {
        while let Some(observation)=origin_receive.recv().await {
            let result=async {
                let profile:HomeConnection=serde_json::from_slice(&std::fs::read(&home_path)?)?;
                crate::scene_origin::send(&running.peer,&profile,&observation).await
            }.await;
            if let Err(e)=result {eprintln!("Home origin observation: {e}");}
        }
    };
    let volume_work = async {
        if hardware != Hardware::Bracketbot {
            std::future::pending::<()>().await;
        }
        let cache = path.with_file_name("semantic-volumes.json");
        loop {
            let work = async {
                let profile: HomeConnection = serde_json::from_slice(&std::fs::read(&home_path)?)?;
                let cancel = tokio_util::sync::CancellationToken::new();
                crate::local_mobile::receive_scene(&running.peer, &profile, &cancel,
                    Duration::from_secs(5), |scene| {
                        save(&cache, &serde_json::json!({
                            "schema":"spatial.robot-volume-input/v1", "authenticated":true,
                            "home_peer_id":profile.peer_id, "home_frame":profile.frame,
                            "robot_peer_id":owner,
                            "received_ns":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64,
                            "volumes":scene.semantic_volumes
                        }))
                    }).await
            };
            if let Err(error) = work.await {
                let _ = save(&cache, &serde_json::json!({"authenticated":false}));
                eprintln!("Home volume subscription: {error}");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    };
    let phone_work = async {
        let mut presence_reader = crate::mobile_pose::PresenceReader::default();
        loop {
            let work = async {
                let profile: HomeConnection = serde_json::from_slice(&std::fs::read(&home_path)?)?;
                let (snapshot, age) = presence_reader.read(&running.peer, &profile).await?;
                Ok::<_, anyhow::Error>((profile, snapshot, age))
            };
            let packet = match tokio::time::timeout(Duration::from_millis(800), work).await {
                Ok(Ok((profile, snapshot, age))) => serde_json::json!({
                    "schema":"spatial.robot-phone-input/v1", "authenticated":true,
                    "home_peer_id":profile.peer_id,"home_frame":profile.frame,
                    "robot_peer_id":running.peer.peer_id().to_string(),
                    "request_age_ms":age.as_millis(),"presence":snapshot.presence,
                    "received_ns":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
                }),
                _ => {
                    presence_reader = Default::default();
                    serde_json::json!({"schema":"spatial.robot-phone-input/v1","authenticated":false,"presence":null})
                },
            };
            if let Err(error) = save(&path.with_file_name("phone-presence.json"), &packet) {
                eprintln!("Phone presence cache: {error}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    // Phone driving is off unless the launcher names the one phone allowed to
    // drive. This only caches Home's command for the local drive controller,
    // which owns every motor decision.
    let drive_peer = std::env::var("SPATIAL_DRIVE_PEER").ok().filter(|v| !v.is_empty());
    let drive_work = async {
        let Some(allowed) = drive_peer else {
            return std::future::pending::<()>().await;
        };
        let own = running.peer.peer_id().to_string();
        let now_ns = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
        let mut reader = crate::mobile_drive::Reader::default();
        let mut last_error = String::new();
        loop {
            // Preferred: Home pushes each accepted command, one hop instead of a
            // polled round trip. An older Home has no stream; fall back to polling.
            let profile = std::fs::read(&home_path).ok().and_then(|b| serde_json::from_slice::<HomeConnection>(&b).ok());
            if let Some(profile) = &profile
                && let Ok(mut stream) = crate::mobile_drive::subscribe(&running.peer, profile).await {
                eprintln!("Drive relay: following Home's pushed commands");
                last_error.clear();
                let mut latest_sequence = (String::new(), 0u64);
                loop {
                    use auki_component_protocol::RemoteObservationEvent;
                    // The phone repeats a held stick ten times a second. Refresh the
                    // cache on silence too, so the controller sees "no command", not a stale file.
                    let event = tokio::time::timeout(Duration::from_millis(250), stream.next()).await;
                    let command = match event {
                        Ok(Ok(Some(RemoteObservationEvent::Observation(o)))) => {
                            let c = &o.payload;
                            if c.target_peer_id != own || c.driver_peer_id != allowed { continue; }
                            if latest_sequence.0 == c.session && c.sequence <= latest_sequence.1 { continue; }
                            latest_sequence = (c.session.clone(), c.sequence);
                            Some(serde_json::json!({"driver_peer_id":c.driver_peer_id,"session":c.session,"sequence":c.sequence,
                                "forward":c.forward,"turn":c.turn,"age_ms":0}))
                        }
                        Ok(Ok(Some(RemoteObservationEvent::Gap(_)))) | Err(_) => None,
                        _ => break, // closed or failed: resubscribe or fall back
                    };
                    let packet = serde_json::json!({
                        "schema":"spatial.robot-drive-input/v1","authenticated":true,"transport":"push",
                        "allowed_driver_peer_id":allowed,"request_age_ms":0,"received_ns":now_ns(),"command":command
                    });
                    if let Err(error) = save(&path.with_file_name("drive-command.json"), &packet) {
                        eprintln!("Drive command cache: {error}");
                    }
                }
                let _ = stream.close().await;
                eprintln!("Drive relay: pushed stream ended; retrying");
            }
            // Polling fallback, for about five seconds before trying the stream again.
            let fallback_until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < fallback_until {
            // Finding the relay tries each profile route in turn and can take
            // seconds; a pinned read must be quick or the command is stale.
            // A pinned read measured 130-390 ms against a live Home, so 350 ms
            // dropped about one read in five.
            let budget = Duration::from_millis(if reader.is_pinned() { 800 } else { 5000 });
            let work = async {
                let profile: HomeConnection = serde_json::from_slice(&std::fs::read(&home_path)?)?;
                reader.read(&running.peer, &profile).await
            };
            let outcome = tokio::time::timeout(budget, work).await;
            let error = match &outcome {
                Ok(Ok(_)) => String::new(),
                Ok(Err(error)) => error.to_string(),
                Err(_) => format!("no reply within {} ms", budget.as_millis()),
            };
            if error != last_error {
                if error.is_empty() { eprintln!("Drive relay: polling commands from Home"); }
                else { eprintln!("Drive relay unavailable: {error}"); }
                last_error = error.clone();
            }
            let received_ns = now_ns();
            let packet = match outcome {
                Ok(Ok((snapshot, elapsed))) => {
                    let command = snapshot.command.filter(|c| c.driver_peer_id == allowed);
                    serde_json::json!({
                        "schema":"spatial.robot-drive-input/v1","authenticated":true,"transport":"poll",
                        "allowed_driver_peer_id":allowed,"request_age_ms":elapsed.as_millis() as u64,
                        "received_ns":received_ns,"command":command
                    })
                }
                _ => {
                    reader = Default::default();
                    serde_json::json!({"schema":"spatial.robot-drive-input/v1","authenticated":false,"received_ns":received_ns,"command":null,"error":error})
                }
            };
            if let Err(error) = save(&path.with_file_name("drive-command.json"), &packet) {
                eprintln!("Drive command cache: {error}");
            }
            // The read itself is the slow part; poll again almost at once.
            tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    };
    let live_work = async {
        loop {
            let snapshot = live.lock().unwrap().snapshot();
            if let Some(snapshot) = snapshot
                && let Err(error) = save(&path.with_file_name("robot-control.json"), &snapshot) {
                eprintln!("Robot control snapshot: {error}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let contributions = async {
        while let Some(c) = contribution_receive.recv().await {
            let work = async {
                let profile:HomeConnection=serde_json::from_slice(&std::fs::read(&home_path)?)?;
                crate::scene_contribution::send(&running.peer,&profile,&c).await
            };
            if let Err(e) = tokio::time::timeout(Duration::from_secs(3),work).await.unwrap_or_else(|_|Err(anyhow!("Contribution timed out"))) {
                eprintln!("Scene contribution: {e}");
            }
        }
    };
    let pose_work = async {
        let mut sender = crate::mobile_pose::PresenceSender::default();
        loop {
            let began = std::time::Instant::now();
            let work = async {
                let profile: HomeConnection = serde_json::from_slice(&std::fs::read(&home_path)?)?;
                let value = live.lock().unwrap().snapshot();
                let update = match value {
                    Some(value) => {
                        let name=std::env::var("SPATIAL_ROBOT_NAME").unwrap_or_else(|_|if hardware==Hardware::Bracketbot { "Bracketbot 0184".into() } else { "Galbot".into() });
                        crate::robot_presence::from_live(&value,&name)?
                    },
                    None => crate::robot_presence::Update { sample:None },
                };
                sender.send(&running.peer,&profile,crate::robot_presence::COMPONENT,&update).await
            };
            let result = tokio::time::timeout(Duration::from_secs(2),work).await;
            let mut status = match result {
                Ok(Ok(())) => serde_json::json!({"status":"accepted"}),
                Ok(Err(e)) => serde_json::json!({"status":"unavailable","error":e.to_string()}),
                Err(_) => serde_json::json!({"status":"unavailable","error":"Pose publication timed out"}),
            };
            status["elapsed_ms"] = serde_json::json!(began.elapsed().as_millis());
            status["catalog_requests"] = serde_json::json!(sender.catalog_requests());
            let _ = save(&path.with_file_name("robot-presence-status.json"), &status);
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };
    let health = async {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if reader.is_finished()
                || sensor_worker.is_finished()
                || mapper.is_finished()
                || writer.is_finished()
            {
                break;
            }
        }
    };
    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{},_=home_work=>{},_=origin_work=>{},_=health=>{},_=contributions=>{},_=phone_work=>{},_=live_work=>{},_=drive_work=>{},_=volume_work=>{},_=pose_work=>{}}
    let _ = child.start_kill();
    let _ = child.wait().await;
    stop.store(true, Ordering::Relaxed);
    let reader_result = if reader.is_finished() {
        reader.await?
    } else {
        reader.abort();
        let _ = reader.await;
        Ok(())
    };
    let sensor_result = sensor_worker.await?;
    let (publisher, map_result) = mapper.await?;
    let disk_result = writer.await?;
    // Persist the final completed map if the writer exited before the mapper.
    if let Some(s) = disk.lock().unwrap().take()
        && let Some(scene) = &s.scene
    {
        save(&path, scene)?;
    }
    let cleanup = publisher.close().await;
    let peer_cleanup = tokio::time::timeout(Duration::from_secs(15), running.shutdown()).await?;
    reader_result
        .and(sensor_result)
        .and(map_result)
        .and(disk_result)
        .and(cleanup)
        .and(peer_cleanup)
}
fn save(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(value)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}
