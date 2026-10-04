//! One peer/runtime; acquisition, sensors, mapping and disk work run independently.
use anyhow::{Result, anyhow, ensure};
use spatial_home::{
    fresh_ingress::FreshIngress,
    local_mapper::{LocalMap, SensorEvent},
    network::{CanonicalScene, HomeConnection, ScenePublisher},
    peer::PeerSettings,
};
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
#[derive(serde::Serialize)]
struct DiskState {
    scene: CanonicalScene,
    catalog: auki_components::CatalogSnapshot,
    status: serde_json::Value,
}
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4,
        "Usage: galbot-runner PEER.json SCENE.json PROFILE.json"
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
    let frame = format!("galbot-local:{session}:zup");
    let mut publisher = ScenePublisher::mount_in_frame(&running.peer, &frame)?;
    let mut local = LocalMap::new(
        &publisher.runtime,
        &running.peer.peer_id().to_string(),
        &session,
    )?;
    local.export_sources(&publisher.endpoint)?;
    let mut connection = HomeConnection::current(&running.peer)?;
    connection.frame = frame.clone();
    save(Path::new(&args[3]), &connection)?;
    if path.exists() {
        std::fs::rename(
            &path,
            path.with_extension(format!("previous-{session}.json")),
        )?;
    }
    let script = std::env::var_os("SPATIAL_LOCAL_CAPTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| "scripts/capture-runner.py".into());
    let mut child=tokio::process::Command::new("python3.8").arg(script)
        .env("PYTHONPATH","/data/galbot/lib:/data/galbot/lib/python3.8.10:/data/galbot/lib/python3/site-packages")
        .env("GALBOT_SENSOR_STATE",path.parent().unwrap())
        .env("GALBOT_CAPTURE_CONTEXT",serde_json::to_string(&serde_json::json!({"clock":spatial_home::model::clock_ref(&spatial_home::model::clock_entry(&running.peer.peer_id().to_string(),&session)),"session_id":session}))?)
        .env("LD_LIBRARY_PATH","/data/galbot/lib").env("OPENBLAS_NUM_THREADS","1").env("OMP_NUM_THREADS","1").env_remove("AUKI_ACCESS_KEY").env_remove("AUKI_APP_SECRET")
        .stdout(std::process::Stdio::piped()).kill_on_drop(true).spawn()?;
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let sensors: Inbox = Default::default();
    let mapping: Inbox = Default::default();
    let stop = Arc::new(AtomicBool::new(false));
    let observed = Arc::new(AtomicBool::new(false));
    let disk = Arc::new(Mutex::new(None::<DiskState>));
    let sensor_status = Arc::new(Mutex::new(serde_json::json!({})));
    let (home_send, home_receive) = std::sync::mpsc::sync_channel(1);
    let reader = {
        let sensors = sensors.clone();
        let mapping = mapping.clone();
        tokio::spawn(async move {
            while let Some(line) = lines.next_line().await? {
                if let Some(raw) = line.strip_prefix("SENSOR ") {
                    ensure!(raw.len() < 16 * 1024 * 1024, "sensor packet exceeds limit");
                    let value: serde_json::Value = serde_json::from_str(raw)?;
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
            let mut mounted =
                std::collections::BTreeMap::<String, spatial_home::galbot_sensors::Sensor>::new();
            let work = (|| -> Result<()> {
                while !stop.load(Ordering::Relaxed) {
                    let batch = sensors.lock().unwrap().drain();
                    for value in batch {
                        if value["kind"] == "register_sensor" {
                            let spec: spatial_home::galbot_sensors::SensorSpec =
                                serde_json::from_value(value["spec"].clone())?;
                            ensure!(!mounted.contains_key(&spec.id), "duplicate sensor");
                            let sensor = spatial_home::galbot_sensors::Sensor::mount(
                                &runtime, &endpoint, spec,
                            )
                            .map_err(|e| anyhow!(e))?;
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
        let stop = stop.clone();
        let observed = observed.clone();
        let disk = disk.clone();
        tokio::task::spawn_blocking(move || {
            let work = (|| -> Result<()> {
                let mut exported = false;
                let mut last_publish = Instant::now() - Duration::from_secs(1);
                let mut last_odom = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    let started = Instant::now();
                    let batch = mapping.lock().unwrap().drain();
                    for value in batch {
                        let event: SensorEvent = serde_json::from_value(value)?;
                        let odom = matches!(event, SensorEvent::Odometry { .. });
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
                    if let Ok(home) = home_receive.try_recv()
                        && let Err(e) = local.accept_home(home)
                    {
                        eprintln!("Home references: {e}");
                    }
                    if last_odom.elapsed() > Duration::from_secs(3) {
                        local.latest_pose = None;
                    }
                    if !exported {
                        exported = local.export_camera_preview(&publisher.endpoint)?;
                    }
                    observed.store(
                        !local.portals()?.scenegraph.anchors.is_empty(),
                        Ordering::Relaxed,
                    );
                    if last_publish.elapsed() >= Duration::from_millis(250) {
                        if let Some(scene) = publisher.publish_local(&local)? {
                            let status = serde_json::json!({"frame":frame,"origin":local.origin,"detail":local.detail,"status":if local.origin.is_some(){"mapping"}else{"waiting_for_odometry"},"mapper_batch_ms":started.elapsed().as_millis(),"coalesced_samples":mapping.lock().unwrap().replaced,"sensors":sensor_status.lock().unwrap().clone()});
                            *disk.lock().unwrap() = Some(DiskState {
                                scene,
                                catalog: publisher.runtime.catalog().snapshot(),
                                status,
                            });
                        }
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
                    save(&path, &s.scene)?;
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
            if observed.load(Ordering::Relaxed) {
                let fetch = async {
                    let profile: HomeConnection =
                        serde_json::from_slice(&std::fs::read(&home_path)?)?;
                    spatial_home::network::fetch_home_scene(&running.peer, &profile).await
                };
                match fetch.await {
                    Ok(s) => {
                        let _ = home_send.try_send(s);
                    }
                    Err(e) => eprintln!("Home reference fetch: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
            } else {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    };
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
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
    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{},_=home_work=>{},_=health=>{}}
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
    if let Some(s) = disk.lock().unwrap().take() {
        save(&path, &s.scene)?;
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
