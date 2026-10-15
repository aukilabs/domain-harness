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
fn sample_capture_clock(mapping: &mut crate::capture_time::NativeMapping, session: &auki_time::SessionClock) -> Result<()> {
    mapping.observe(session, || auki_time::Clock::read_to_ns(&auki_time::SystemClock) as u64)
}

fn bbos_native_clock(owner: &str, session: &str) -> auki_registry::ClockRegistryEntry {
    auki_registry::ClockRegistryEntry {
        peer_id: owner.into(), session_id: session.into(),
        clock_id: format!("{owner}/{session}/bbos-host-realtime"),
        body: auki_registry::ClockBody::UtcClock(auki_registry::ClockMeta {
            unit: "nanoseconds".into(), monotonic: false,
            epoch: Some("1970-01-01T00:00:00Z".into()), scope: auki_registry::Scope::DeviceLocal,
        }),
    }
}

/// Exact capture correspondences for the local BBOS control/history adapter.
/// SDK publications remain session-monotonic. The Python helpers read native
/// BBOS times, so retain original values instead of approximating an inverse.
#[derive(Default)]
struct NativeCaptureTimes {
    by_session: std::collections::BTreeMap<u64, u64>,
    by_native: std::collections::BTreeMap<u64, u64>,
    retained: std::collections::BTreeSet<u64>,
}
impl NativeCaptureTimes {
    fn convert(&mut self, mapping: &crate::capture_time::NativeMapping, native: u64) -> Result<u64> {
        if let Some(session) = self.by_native.get(&native) { return Ok(*session); }
        let session = mapping.convert(native)?;
        ensure!(self.by_session.get(&session).is_none_or(|old| *old == native), "Ambiguous native capture time");
        self.by_session.insert(session, native);
        self.by_native.insert(native, session);
        Ok(session)
    }
    fn prune(&mut self, now: u64) -> Result<()> {
        let cutoff = now.saturating_sub(10_000_000_000);
        self.by_session.retain(|session, _| *session >= cutoff || self.retained.contains(session));
        self.by_native.retain(|_, session| self.by_session.contains_key(session));
        ensure!(self.by_session.len() <= 131_072, "Native capture correspondence budget exceeded");
        Ok(())
    }
    fn control(&mut self, mut value: serde_json::Value,
        source: &auki_components::ClockReference, native_definition: &auki_registry::ClockRegistryEntry) -> Result<serde_json::Value> {
        fn convert(value: &mut serde_json::Value, table: &std::collections::BTreeMap<u64,u64>,
            retained: &mut std::collections::BTreeSet<u64>, source: &serde_json::Value, native: &serde_json::Value) -> Result<()> {
            if value == source { *value = native.clone(); return Ok(()); }
            let timestamp = |value: &mut serde_json::Value, retained: &mut std::collections::BTreeSet<u64>| -> Result<()> {
                if let Some(session) = value.as_u64().filter(|v| *v > 0) {
                    let native = table.get(&session).ok_or_else(|| anyhow!("Missing original native capture timestamp"))?;
                    retained.insert(session);
                    *value = (*native).into();
                }
                Ok(())
            };
            match value {
                serde_json::Value::Object(fields) => for (key, value) in fields {
                    if key == "provenance" {
                        // Original SDK observation references retain their
                        // exact clock/time even in this native control view.
                        continue;
                    } else if key == "retained_depth_timestamps" {
                        if let Some(values) = value.as_array_mut() { for value in values { timestamp(value, retained)?; } }
                    } else if key == "timestamp_ns" || key.ends_with("_timestamp_ns") || key == "trajectory_correction_ns" {
                        timestamp(value, retained)?;
                    } else { convert(value, table, retained, source, native)?; }
                },
                serde_json::Value::Array(values) => for value in values { convert(value, table, retained, source, native)?; },
                _ => {},
            }
            Ok(())
        }
        let mut retained = std::collections::BTreeSet::new();
        let source = serde_json::to_value(source)?;
        let native = serde_json::to_value(crate::model::clock_ref(native_definition))?;
        convert(&mut value, &self.by_session, &mut retained, &source, &native)?;
        value["local_clock_adapter"] = serde_json::json!({"sdk_clock":source,"native_clock":native,
            "native_clock_definition":native_definition,"method":"original_capture_correspondence"});
        self.retained = retained;
        Ok(value)
    }
}

/// This bridge's BBOS adapter explicitly stamps host CLOCK_REALTIME. Convert
/// each original capture through measured clock samples, including nested
/// historical corrections. Never derive an offset from packet arrival time.
fn map_capture_times(value: &mut serde_json::Value, mapping: &crate::capture_time::NativeMapping, times: &mut NativeCaptureTimes) -> Result<()> {
    fn timestamp(value: &mut serde_json::Value, mapping: &crate::capture_time::NativeMapping, times: &mut NativeCaptureTimes) -> Result<()> {
        if let Some(t) = value.as_u64() { *value = times.convert(mapping, t)?.into(); }
        Ok(())
    }
    if let Some(t) = value.get_mut("timestamp_ns") { timestamp(t, mapping, times)?; }
    if let Some(sample) = value.get_mut("sample") {
        for field in ["timestamp_ns", "rgb_timestamp_ns", "transform_timestamp_ns"] {
            if let Some(t) = sample.get_mut(field) { timestamp(t, mapping, times)?; }
        }
    }
    if let Some(portals) = value.get_mut("corrected_portals").and_then(serde_json::Value::as_object_mut) {
        for pose in portals.values_mut() {
            if let Some(t) = pose.get_mut("timestamp_ns") { timestamp(t, mapping, times)?; }
        }
    }
    if let Some(depth) = value.get_mut("corrected_depth").and_then(serde_json::Value::as_object_mut) {
        let mapped = depth.iter().map(|(t, pose)| Ok((times.convert(mapping, t.parse()?)?.to_string(), pose.clone())))
            .collect::<Result<serde_json::Map<String, serde_json::Value>>>()?;
        *depth = mapped;
    }
    Ok(())
}

pub async fn run() -> Result<()> {
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
    let family = crate::local_mapper::BRACKETBOT_FAMILY;
    let frame = format!("{family}:{session}:zup");
    let owner = running.peer.peer_id().to_string();
    let session_clock = auki_time::SessionClock::new(&owner, &session, "session");
    let mut publisher = ScenePublisher::mount_in_frame_with_clock(&running.peer, &frame, session_clock.clone())?;
    let registry_definitions = crate::product_registry::Definitions::default();
    publisher.register_clocks(&registry_definitions)?;
    let capture_clock = session_clock.registry_entry();
    crate::product_registry::clock(&registry_definitions, &capture_clock)?;
    let registry = crate::product_registry::mount(&running.peer, publisher.runtime.clone(), registry_definitions)?;
    let mut local = LocalMap::new_for_robot_with_clock(
        &publisher.runtime,
        &owner,
        capture_clock.clone(),
        family,
        crate::rgbd::BRACKETBOT_FRAME,
        serde_json::from_str(include_str!(
            "../config/humanoid-hub-conference/portals.json"
        ))?,
        Some(session_clock.clone()),
    )?;
    if let Some(path) = std::env::var_os("SPATIAL_PORTAL_SIZES") {
        local.set_portal_sizes(crate::portal_sizes::PortalSizes::load(path)?)?;
    }
    local.export_sources(&publisher.endpoint)?;
    local.copy_native_scans();
    let contribution_clock = capture_clock.clone();
    let contribution_owner = owner.clone();
    let (contribution_send, mut contribution_receive) = tokio::sync::mpsc::channel::<crate::scene_contribution::Contribution>(2);
    let mut connection = HomeConnection::current(&running.peer)?;
    connection.frame = family.into();
    save(Path::new(&args[3]), &connection)?;
    if path.exists() {
        std::fs::rename(
            &path,
            path.with_extension(format!("previous-{session}.json")),
        )?;
    }
    let script = std::env::var_os("SPATIAL_LOCAL_CAPTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| "scripts/capture-bracketbot.py".into());
    let python = std::env::var_os("SPATIAL_CAPTURE_PYTHON").unwrap_or_else(|| "python3".into());
    let context = serde_json::to_string(&serde_json::json!({
        "clock":crate::model::clock_ref(&capture_clock),
        "session_id":session,
        "control_state_path":path.with_file_name("robot-control.json"),
        "camera_frame":crate::rgbd::BRACKETBOT_FRAME,
        "camera_frame_definition":auki_registry::FrameRegistryEntry::ros_optical(&owner,crate::rgbd::BRACKETBOT_FRAME),
        "base_frame_definition":auki_registry::FrameRegistryEntry::in_convention(&owner,"robot.base",auki_registry::CoordinateConvention::ZUpRightForward),
    }))?;
    let mut command = tokio::process::Command::new(python);
    command.arg("scripts/bracketbot-python.py");
    let mut child = command
        .arg(script)
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
    let native_times = Arc::new(Mutex::new(NativeCaptureTimes::default()));
    // A local IPC clock, distinct from the fixed session-start UTC anchor.
    // BBOS realtime may be adjusted; the native sampler detects those changes.
    let native_clock = bbos_native_clock(&owner, &session);
    let live = Arc::new(Mutex::new(live::LiveState::new(true)));
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
        let native_times = native_times.clone();
        tokio::spawn(async move {
            let mut clocks = crate::capture_time::NativeMapping::default();
            let mut clock_tick = tokio::time::interval(Duration::from_millis(100));
            loop {
                let line = tokio::select! {
                    _ = clock_tick.tick() => {
                        sample_capture_clock(&mut clocks, &session_clock)?;
                        continue;
                    },
                    line = lines.next_line() => match line? { Some(line) => line, None => break },
                };
                if let Some(raw) = line.strip_prefix("SENSOR ") {
                    ensure!(raw.len() < 16 * 1024 * 1024, "sensor packet exceeds limit");
                    let mut value: serde_json::Value = serde_json::from_str(raw)?;
                    if value["kind"] != "register_sensor" {
                        sample_capture_clock(&mut clocks, &session_clock)?;
                        let mut times = native_times.lock().unwrap();
                        times.prune(session_clock.now_ns())?;
                        if let Err(error) = map_capture_times(&mut value, &clocks, &mut times) {
                            if matches!(value["kind"].as_str(), Some("map_correction" | "trajectory_correction" | "correction_pending")) {
                                return Err(error.context("Cannot map a coordinate-reset event into this session"));
                            }
                            eprintln!("Capture outside measured clock history: {error}");
                            continue;
                        }
                    }
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
                std::collections::BTreeMap::<String, crate::runner_sensors::Sensor>::new();
            let work = (|| -> Result<()> {
                while !stop.load(Ordering::Relaxed) {
                    let batch = sensors.lock().unwrap().drain();
                    for value in batch {
                        if value["kind"] == "register_sensor" {
                            let spec: crate::runner_sensors::SensorSpec =
                                serde_json::from_value(value["spec"].clone())?;
                            ensure!(!mounted.contains_key(&spec.id), "duplicate sensor");
                            let mut sensor =
                                crate::runner_sensors::Sensor::mount(&runtime, &endpoint, spec)
                                    .map_err(|e| anyhow!(e))?;
                            if matches!(
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
        let native_clock = native_clock.clone();
        let stop = stop.clone();
        let home_status = home_status.clone();
        let disk = disk.clone();
        tokio::task::spawn_blocking(move || {
            let work = (|| -> Result<()> {
                let mut last_publish = Instant::now() - Duration::from_secs(1);
                let mut last_odom = Instant::now();
                let mut last_joints = Instant::now();
                let mut last_health = Instant::now();
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
                        let joints = matches!(event, SensorEvent::Joints { .. });
                        let health = matches!(event, SensorEvent::Health(_));
                        match local.accept(event) {
                            Ok(()) => {
                                if odom {
                                    last_odom = Instant::now();
                                }
                                if joints {
                                    last_joints = Instant::now();
                                }
                                if health {
                                    last_health = Instant::now();
                                }
                            }
                            Err(e) => {
                                local.detail = format!("Observation rejected: {e}");
                                eprintln!("{}", local.detail);
                            }
                        }
                    }
                    // Joints from an arm reader that stopped would freeze the model.
                    if last_joints.elapsed() > Duration::from_secs(3) {
                        local.latest_joints = None;
                    }
                    if last_health.elapsed() > Duration::from_secs(5) {
                        local.latest_health = None;
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
                        let status = serde_json::json!({"frame":frame,"origin":local.origin,"detail":local.detail,"capture_status":capture_status,"scan_progress":local.scan_progress(),"status":if local.latest_pose.is_some(){"mapping"}else{"waiting_for_odometry"},"mapper_batch_ms":started.elapsed().as_millis(),"coalesced_samples":mapping.lock().unwrap().replaced,"sensors":sensor_status.lock().unwrap().clone(),"home_reference":home_status.lock().unwrap().clone(),"home_reference_applied":local.home_portals.is_some(),
                            "local_control_clock":crate::model::clock_ref(&native_clock)});
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
        let cache = path.with_file_name("semantic-volumes.json");
        loop {
            let work = async {
                let profile: HomeConnection = serde_json::from_slice(&std::fs::read(&home_path)?)?;
                let cancel = tokio_util::sync::CancellationToken::new();
                crate::local_mobile::receive_scene(&running.peer, &profile, &cancel,
                    Some(Duration::from_secs(5)), |scene| {
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
            if let Some(snapshot) = snapshot {
                // This local IPC file is for BBOS history and motion helpers,
                // which explicitly operate in BBOS's native realtime clock.
                // Never export this derived view as an SDK Product.
                let snapshot = native_times.lock().unwrap().control(snapshot, &crate::model::clock_ref(&capture_clock), &native_clock);
                if let Err(error) = snapshot.and_then(|snapshot| save(&path.with_file_name("robot-control.json"), &snapshot)) {
                    eprintln!("Robot control snapshot: {error}");
                }
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
                        let name=std::env::var("SPATIAL_ROBOT_NAME").unwrap_or_else(|_| "Bracketbot 0184".into());
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
    let registry_cleanup = registry.close().await;
    let cleanup = publisher.close().await;
    let peer_cleanup = tokio::time::timeout(Duration::from_secs(15), running.shutdown()).await?;
    reader_result
        .and(registry_cleanup.map_err(Into::into))
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

#[cfg(test)]
mod clock_tests {
    use super::*;
    #[test]
    fn capture_mapping_preserves_delay_and_maps_nested_corrections() -> Result<()> {
        let mut mapping = crate::capture_time::NativeMapping::default();
        let mut times = NativeCaptureTimes::default();
        mapping.observe_pair(100, 1100, 100)?;
        mapping.observe_pair(300, 1300, 300)?;
        let mut camera = serde_json::json!({"kind":"camera","sample":{
            "timestamp_ns":1150,"rgb_timestamp_ns":1150,"transform_timestamp_ns":1150}});
        map_capture_times(&mut camera, &mapping, &mut times)?;
        assert_eq!(camera["sample"]["timestamp_ns"], 150);
        assert_eq!(camera["sample"]["rgb_timestamp_ns"], 150);
        assert_eq!(camera["sample"]["transform_timestamp_ns"], 150);
        let mut correction = serde_json::json!({"kind":"trajectory_correction","timestamp_ns":1250,
            "corrected_portals":{"marker":{"timestamp_ns":1150}},"corrected_depth":{"1200":{}}});
        map_capture_times(&mut correction, &mapping, &mut times)?;
        assert_eq!(correction["timestamp_ns"], 250);
        assert_eq!(correction["corrected_portals"]["marker"]["timestamp_ns"], 150);
        assert!(correction["corrected_depth"].get("200").is_some());
        let mut stale = serde_json::json!({"kind":"odometry","timestamp_ns":1050});
        assert!(map_capture_times(&mut stale, &mapping, &mut times).is_err());
        Ok(())
    }
    #[test]
    fn local_control_preserves_original_bbos_times_and_pins_only_referenced_history() -> Result<()> {
        let mut mapping = crate::capture_time::NativeMapping::default();
        mapping.observe_pair(100, 1100, 100)?;
        mapping.observe_pair(300, 1300, 300)?;
        let mut times = NativeCaptureTimes::default();
        assert_eq!(times.convert(&mapping, 1150)?, 150);
        assert_eq!(times.convert(&mapping, 1200)?, 200);
        let clock = auki_time::SessionClock::new("peer", "test", "session");
        let source = crate::model::clock_ref(&clock.registry_entry());
        let native = bbos_native_clock("peer", "test");
        let native_ref = crate::model::clock_ref(&native);
        assert_ne!(native, crate::capture_time::Contract::local(clock.registry_entry()).to_clock);
        let input = serde_json::json!({"registries":{"local_mapping":{
            "robot_pose":{"capture_timestamp_ns":150,"clock":source},
            "retained_depth_timestamps":[150],
            "origin":{"capture_timestamp_ns":150,"capture_clock":source},
            "provenance":{"timestamp_ns":150,"clock":source}
        }}});
        let control = times.control(input.clone(), &source, &native)?;
        assert_eq!(control["registries"]["local_mapping"]["robot_pose"]["capture_timestamp_ns"], 1150);
        assert_eq!(control["registries"]["local_mapping"]["robot_pose"]["clock"], serde_json::to_value(&native_ref)?);
        assert_eq!(control["registries"]["local_mapping"]["origin"]["capture_clock"], serde_json::to_value(&native_ref)?);
        assert_eq!(control["registries"]["local_mapping"]["retained_depth_timestamps"][0], 1150);
        assert_eq!(control["registries"]["local_mapping"]["provenance"], input["registries"]["local_mapping"]["provenance"]);
        assert_eq!(input["registries"]["local_mapping"]["robot_pose"]["capture_timestamp_ns"], 150,
            "SDK state must not be rewritten by the local control adapter");
        times.prune(11_000_000_000)?;
        assert_eq!(times.by_session.len(), 1);
        // Even after bounded clock history expires, an optimized-pose reply for
        // a pinned Portal must recover its exact original SDK capture key.
        assert_eq!(times.convert(&crate::capture_time::NativeMapping::default(), 1150)?, 150);
        let missing = serde_json::json!({"capture_timestamp_ns":200});
        assert!(times.control(missing, &source, &native).is_err());
        Ok(())
    }
}
