//! Connection-only diagnostic; --poses adds exactly one typed BBOS pose Product
//! and a `point_at` Operable over the frames this peer relates.
mod alignment;
mod arm;
mod body;
mod camera;
mod diagnostic_log;
#[path = "../../../src/camera_frame.rs"]
mod camera_frame;
mod depth;
mod drive;
#[path = "../../../src/drive_contract.rs"]
mod drive_contract;
#[path = "../../../src/component_enabled.rs"]
mod component_enabled;
#[path = "../../../src/depth_points.rs"]
mod depth_points;
#[path = "../../../src/compact_pose.rs"]
mod compact_pose;
#[path = "../../../src/fisheye.rs"]
mod fisheye;
mod home;
mod nav;
#[path = "../../../src/navigate.rs"]
mod navigate;
#[path = "../../../src/point_at.rs"]
mod point_at;
mod pointer;
mod poses;
#[path = "../../../src/robot_state.rs"]
mod robot_state;
#[path = "../../../src/qr_decode.rs"]
mod qr_decode;
use anyhow::{Context, Result, ensure};
use auki_component_protocol::{CatalogResponse, ComponentProtocolClient};
use auki_sdk::{
    AukiPeerBootstrap, AukiPeerConfig, AuthClient, AuthEnvironment, Credentials, DomainSelection,
    Multiaddr, PeerId,
};
use serde::Deserialize;
use std::{
    path::Path,
    time::{Duration, Instant},
};
use tokio::io::AsyncBufReadExt;

#[derive(Deserialize)]
struct Config {
    domain_id: String,
    api_url: String,
    dds_url: String,
    dms_url: String,
    login: String,
}
#[derive(Clone, Deserialize)]
struct Home {
    schema: String,
    domain_id: String,
    peer_id: String,
    routes: Vec<String>,
    #[serde(default)]
    frame: String,
    #[serde(default)]
    product_id: String,
}

fn read<T: serde::de::DeserializeOwned>(path: &str) -> Result<T> {
    let bytes = std::fs::read(path)?;
    ensure!(bytes.len() <= 32768, "configuration too large");
    Ok(serde_json::from_slice(&bytes)?)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let _logging = diagnostic_log::start()?;
    let args: Vec<_> = std::env::args().collect();
    let options = args.get(5..).unwrap_or_default();
    let usage = "usage: auki-connection-diagnostic PEER_CONFIG HOME_CONNECTION DIAGNOSTIC_IDENTITY [--poses|--portals|--points|--check-home] [--arm-plan|--arm-motion] [--drive] [--navigate]";
    ensure!(
        args.len() == 4
            || (args.len() == 5
                && matches!(args[4].as_str(), "--poses" | "--portals" | "--points" | "--check-home"))
            || (args.len() >= 6
                && matches!(args[4].as_str(), "--poses" | "--portals" | "--points")
                && options.iter().all(|o| matches!(o.as_str(), "--arm-plan" | "--arm-motion" | "--drive" | "--navigate"))
                && options.iter().filter(|o| o.starts_with("--arm-")).count() <= 1
                && options.iter().filter(|o| *o == "--drive").count() <= 1
                && options.iter().filter(|o| *o == "--navigate").count() <= 1),
        "{usage}"
    );
    // point_at plans with the arm adapter (--arm-plan) and moves (--arm-motion).
    let arm = options.iter().find(|o| o.starts_with("--arm-")).map(|v| v == "--arm-motion");
    // Teleop: Home's own peer drives the base through its scene's drive relay.
    let drive = options.iter().any(|o| o == "--drive");
    // Navigation: Home's own peer sends waypoints the BBOS nav app drives.
    let navigate = options.iter().any(|o| o == "--navigate");
    let check_home = args.get(4).is_some_and(|v| v == "--check-home");
    let publish_poses = args.len() >= 5 && !check_home;
    let points = args.get(4).is_some_and(|v| v == "--points");
    let portals = points || args.get(4).is_some_and(|v| v == "--portals");
    let config: Config = read(&args[1])?;
    let home: Home = read(&args[2])?;
    ensure!(
        !portals || (!home.frame.is_empty() && !home.product_id.is_empty()),
        "Portal mode requires a scene-specific Home connection profile"
    );
    ensure!(
        home.schema == "spatial-harness.home-connection/v1",
        "unknown Home profile"
    );
    let drive_scene = drive
        .then(|| drive::scene_id(&home.frame).map(str::to_owned).context("Driving needs a scene-specific Home connection profile"))
        .transpose()?;
    ensure!(home.domain_id == config.domain_id, "Domain mismatch");
    ensure!(
        !home.routes.is_empty() && home.routes.len() <= 16,
        "Home needs 1–16 routes"
    );
    let expected: PeerId = home.peer_id.parse()?;
    let routes: Vec<Multiaddr> = home
        .routes
        .iter()
        .map(|r| r.parse())
        .collect::<Result<_, _>>()?;
    let domain = home.domain_id.parse()?;
    let env = |key| std::env::var(key).with_context(|| format!("missing {key}"));
    let credentials = match config.login.as_str() {
        "app" => Credentials::app(env("AUKI_ACCESS_KEY")?, env("AUKI_APP_SECRET")?),
        "user" => Credentials::user_password(env("AUKI_EMAIL")?, env("AUKI_PASSWORD")?),
        _ => anyhow::bail!("login must be app or user"),
    };
    println!("disconnected");
    let mut peer_config = AukiPeerConfig::new(&config.dms_url)?.direct_only();
    if publish_poses {
        let ip: std::net::Ipv4Addr = env("DIAGNOSTIC_LAN_IP")?.parse()?;
        ensure!(!ip.is_unspecified() && !ip.is_loopback(), "LAN IP required");
        let route: Multiaddr = format!("/ip4/{ip}/tcp/4331").parse()?;
        peer_config = peer_config
            .with_listen_addresses([route.clone()])?
            .with_advertised_direct_routes([route])?;
    }
    let mut bootstrap = AukiPeerBootstrap::authenticate(
        AuthClient::new(AuthEnvironment::new(&config.api_url, &config.dds_url)?)?,
        credentials,
        peer_config,
    )
    .await?;
    if publish_poses {
        bootstrap = bootstrap.with_dds_tracker(auki_sdk::DdsTrackerMode::DiscoverAndAdvertise);
    }
    let peer = match bootstrap
        .start_persistent_peer(DomainSelection::new(domain), Path::new(&args[3]))
        .await
    {
        Ok(peer) => peer,
        Err(error) => {
            bootstrap.session().close().await;
            return Err(error.into());
        }
    };
    crate::diagnostic_log!(
        "Diagnostic peer {} -> Home {} (SDK PR 425)",
        peer.peer_id(),
        expected
    );
    if check_home {
        let client = ComponentProtocolClient::new(peer.protocols());
        let registry = auki_protocols::registry::RegistryClient::new(peer.protocols());
        let mut result = Err(anyhow::anyhow!("No reachable Home route"));
        for route in &routes {
            result = match tokio::time::timeout(
                Duration::from_secs(10),
                home::fetch(&client, &registry, &home, route.clone()),
            )
            .await
            {
                Ok(value) => value,
                Err(_) => Err(anyhow::anyhow!("Home validation timed out")),
            };
            if result.is_ok() {
                break;
            }
        }
        peer.shutdown().await?;
        bootstrap.session().close().await;
        let verified = result?;
        println!(
            "Verified Home scene {} with {} portals through libp2p and exact Registry definitions",
            verified.frame.frame_id,
            verified.portals.scenegraph.anchors.len()
        );
        return Ok(());
    }
    let mut publisher = if publish_poses {
        Some(poses::Publisher::mount(&peer)?)
    } else {
        None
    };
    let known = std::sync::Arc::new(std::sync::Mutex::new(pointer::Frames::default()));
    // Every arm adapter phase, for the bbos-pointing status and robot-side alerts.
    let (pointing_tx, mut pointing_rx) = tokio::sync::mpsc::unbounded_channel::<arm::Reply>();
    let mut pointing_arm: Option<(String, String)> = None;
    let mut pointing_open = true;
    if let Some(p) = &publisher {
        known.lock().unwrap().insert(
            &[&p.contract.from_frame, &p.contract.to_frame],
            pointer::Transform { contract: p.compact(), product: p.product() },
        );
    }
    let _navigator = match (&publisher, navigate) {
        (Some(p), true) => Some(nav::Navigator::mount(p, known.clone(), home.peer_id.clone())?),
        _ => None,
    };
    // With navigation, Home's point_at may turn the robot to face a target first.
    let _pointer = match &publisher {
        Some(p) => Some(pointer::Pointer::mount(p, known.clone(), arm, pointing_tx,
            _navigator.as_ref().map(|n| n.turner()))?),
        None => None,
    };
    let mut input = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let (camera_tx, mut camera_rx) =
        tokio::sync::watch::channel(None::<(camera::Calibration, u64, Vec<u8>)>);
    let camera_task = portals.then(|| {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let work = async {
                let mut child = tokio::process::Command::new("python3")
                    .args([
                        "scripts/bracketbot-python.py",
                        "scripts/diagnostic-camera.py",
                    ])
                    .env_remove("AUKI_ACCESS_KEY")
                    .env_remove("AUKI_APP_SECRET")
                    .env_remove("AUKI_EMAIL")
                    .env_remove("AUKI_PASSWORD")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()?;
                let mut reader = tokio::io::BufReader::new(child.stdout.take().unwrap());
                let length=tokio::time::timeout(Duration::from_secs(30),reader.read_u32_le()).await?? as usize;
                ensure!(length>0 && length<=16384,"Camera configuration too large");
                let mut first=vec![0;length];reader.read_exact(&mut first).await?;
                let first=String::from_utf8(first)?;
                let calibration: camera::Calibration = serde_json::from_str(&first)?;
                calibration.validate()?;
                loop {
                    let timestamp_ns=reader.read_u64_le().await?;
                    let length=reader.read_u32_le().await? as usize;
                    ensure!(length>0 && length<=2*1024*1024,"Camera IPC frame too large");
                    let mut bytes=vec![0;length];reader.read_exact(&mut bytes).await?;
                    camera_tx.send_replace(Some((calibration.clone(), timestamp_ns, bytes)));
                }
                #[allow(unreachable_code)]
                Ok::<(), anyhow::Error>(())
            };
            if let Err(e) = work.await {
                crate::diagnostic_log!("Camera capture failed: {e}");
            }
        })
    });
    let mut camera_publisher: Option<camera::Camera> = None;
    let mut camera_open = portals;
    let (depth_task, mut depth_rx,depth_demand) = if points {
        let (task,rx,demand)=depth::start(); (Some(task),rx,demand)
    } else { (None,tokio::sync::watch::channel(None).1,tokio::sync::watch::channel(false).0) };
    let mut depth_open=points;
    let mut depth_publisher:Option<depth::Depth>=None;
    // Arm and battery state need only poses: the body sits on the pose base frame.
    let (body_task, mut body_rx) = if publish_poses {
        let (task,rx)=body::start(); (Some(task),rx)
    } else { (None,tokio::sync::mpsc::channel(1).1) };
    let mut body_open=publish_poses;
    let mut body_publisher:Option<body::Body>=None;
    let (home_tx, home_rx) = tokio::sync::watch::channel(None::<home::Verified>);
    let home_task = if portals {
        let client = ComponentProtocolClient::new(peer.protocols());
        let registry = auki_protocols::registry::RegistryClient::new(peer.protocols());
        let home = home.clone();
        let routes = routes.clone();
        Some(tokio::spawn(async move {
            let mut previous_error = String::new();
            loop {
                let mut result = None;
                let mut error = String::new();
                for route in &routes {
                    match tokio::time::timeout(
                        Duration::from_secs(5),
                        home::fetch(&client, &registry, &home, route.clone()),
                    )
                    .await
                    {
                        Ok(Ok(value)) => {
                            result = Some(value);
                            break;
                        }
                        Ok(Err(e)) => error = e.to_string(),
                        Err(_) => error = "Home scene/Registry request timed out".into(),
                    }
                }
                if result.is_none() && error != previous_error {
                    crate::diagnostic_log!("Home alignment input unresolved: {error}");
                }
                previous_error = error;
                home_tx.send_replace(result);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }))
    } else {
        None
    };
    let mut alignment: Option<alignment::Worker> = None;
    let mut alignment_error = String::new();
    let client = ComponentProtocolClient::new(peer.protocols());
    let (tx, rx) = tokio::sync::watch::channel(None::<Instant>);
    // An essential helper that exits ends this run; the controller that started
    // it (bracketbot_slam_led_diagnostic.py) restarts it, which respawns them all.
    let mut failure: Option<String> = None;
    let mut drive_task = match drive_scene {
        Some(scene) => {
            let helper = drive::Helper::start(&home.peer_id)?;
            let task = tokio::spawn(drive::follow(
                ComponentProtocolClient::new(peer.protocols()),
                expected,
                routes.clone(),
                scene,
                peer.peer_id().to_string(),
            ));
            Some((task, helper))
        }
        None => None,
    };
    let requests = tokio::spawn(async move {
        let mut revision = None;
        loop {
            let mut success = false;
            for route in &routes {
                match tokio::time::timeout(
                    Duration::from_secs(3),
                    client.catalog_exact(expected, route.clone(), revision),
                )
                .await
                {
                    Ok(Ok(CatalogResponse::Snapshot { snapshot })) => {
                        revision = Some(snapshot.revision);
                        success = true;
                    }
                    Ok(Ok(CatalogResponse::Unchanged { revision: value })) => {
                        revision = Some(value);
                        success = true;
                    }
                    _ => {}
                }
                if success {
                    break;
                }
            }
            tx.send_replace(success.then(Instant::now));
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut demand_tick=tokio::time::interval(Duration::from_millis(100));
    demand_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _=demand_tick.tick(),if depth_open=>{
                let wanted=depth_publisher.as_ref().is_some_and(|d|d.wants_capture(&publisher.as_ref().unwrap().endpoint));
                depth_demand.send_if_modified(|old|{if *old==wanted {false}else{*old=wanted;true}});
            },
            changed=depth_rx.changed(),if depth_open => {
                if changed.is_err() {
                    depth_open=false;
                    if let Some(d)=depth_publisher.take(){d.close(publisher.as_ref().unwrap())?;}
                    crate::diagnostic_log!("Depth unavailable; pose and camera diagnostic continue");
                    continue;
                }
                let sample=depth_rx.borrow_and_update().clone();
                if let Some((config,sample))=sample {
                    let result=(|| {
                        if depth_publisher.is_none(){depth_publisher=Some(depth::Depth::mount(publisher.as_ref().unwrap(),config,expected.to_string())?);}
                        if let Some(sample)=sample {depth_publisher.as_mut().unwrap().publish(sample,&publisher.as_ref().unwrap().endpoint)?;}
                        Ok::<(),anyhow::Error>(())
                    })();
                    if let Err(e)=result {crate::diagnostic_log!("Depth publish failed: {e}");}
                }
            },
            reply=pointing_rx.recv(),if pointing_open => {
                // Closed when no arm adapter runs; stop polling it.
                let Some(reply)=reply else { pointing_open=false; continue };
                let Some(request_id)=reply.request_id.clone() else { continue };
                if let Some(arm)=&reply.arm { pointing_arm=Some((request_id.clone(),arm.clone())); }
                let arm=pointing_arm.as_ref().filter(|(id,_)|*id==request_id).map(|(_,arm)|arm.clone());
                if matches!(reply.phase.as_str(),"rejected"|"fault") {
                    // The LED controller flashes, and speaks when it can, on this line.
                    println!("alert {}",reply.code.as_deref().unwrap_or("other"));
                }
                if let Some(p)=publisher.as_ref() {
                    let result=(|| {
                        if body_publisher.is_none(){body_publisher=Some(body::Body::mount(p)?);}
                        body_publisher.as_mut().unwrap().publish_pointing(robot_state::Pointing {
                            request_id,phase:reply.phase,arm,reason:reply.reason,code:reply.code})
                    })();
                    if let Err(e)=result {eprintln!("Pointing status publish failed: {e}");}
                }
            },
            sample=body_rx.recv(),if body_open => {
                let Some(sample)=sample else {
                    body_open=false;
                    if let Some(b)=body_publisher.take(){b.close(publisher.as_ref().unwrap())?;}
                    eprintln!("Robot state unavailable; pose diagnostic continues");
                    continue;
                };
                let result=(|| {
                    if body_publisher.is_none(){body_publisher=Some(body::Body::mount(publisher.as_ref().unwrap())?);}
                    body_publisher.as_mut().unwrap().publish(sample)
                })();
                if let Err(e)=result {eprintln!("Robot state publish failed: {e}");}
            },
            changed=camera_rx.changed(),if camera_open => {
                if changed.is_err(){camera_open=false;alignment=None;camera_publisher=None;crate::diagnostic_log!("Camera unavailable; pose diagnostic continues");continue;}
                let sample=camera_rx.borrow_and_update().clone();
                if let Some((calibration,t,jpeg))=sample {
                    if camera_publisher.is_none(){
                        match camera::Camera::mount(publisher.as_ref().unwrap(),calibration) {
                            Ok(camera)=>{let (frame,transform)=camera.extrinsic();known.lock().unwrap().insert(&[&frame],transform);camera_publisher=Some(camera)},
                            Err(error)=>{camera_open=false;crate::diagnostic_log!("Camera unavailable: {error}; pose diagnostic continues");continue;}
                        }
                    }
                    if let Err(e)=camera_publisher.as_mut().unwrap().publish(t,jpeg){crate::diagnostic_log!("Camera publish failed: {e}");}
                }
            },
            line = input.next_line(), if publish_poses => {
                match line {
                    Ok(Some(line)) if line.len() <= 4096 => {
                        match serde_json::from_str::<poses::Sample>(&line).map_err(anyhow::Error::from)
                            .and_then(|s| publisher.as_mut().unwrap().publish(s)) {
                            Ok(()) => {}, Err(error) => { crate::diagnostic_log!("Pose input failed: {error}"); break; }
                        }
                    },
                    _ => { crate::diagnostic_log!("Pose input ended or invalid; stopping diagnostic"); break; }
                }
            },
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
            _ = peer.wait_stopped() => break,
            _ = tick.tick() => {
                let gone = if _pointer.as_ref().is_some_and(|p| p.arm_gone()) { Some("Arm adapter") }
                    else if _navigator.as_ref().is_some_and(|n| n.gone()) { Some("Nav adapter") }
                    else if drive_task.as_mut().is_some_and(|(_, helper)| helper.exited()) { Some("Drive helper") }
                    else { None };
                if let Some(which) = gone {
                    let message = format!("{which} exited; stopping so the controller restarts this diagnostic");
                    crate::diagnostic_log!("{message}");
                    failure = Some(message);
                    break;
                }
                if let (Some(publisher),Some(camera),Some(home))=(&publisher,&camera_publisher,home_rx.borrow().clone()) {
                    if alignment.as_ref().is_some_and(|a|a.target!=(&home.frame).into()) {alignment=None;}
                    if alignment.is_none() {
                        match alignment::Worker::mount(publisher,camera,home,home_rx.clone()) {
                            Ok(worker)=>{let (frame,transform)=worker.transform();known.lock().unwrap().insert(&[&frame],transform);alignment=Some(worker);alignment_error.clear();},
                            Err(e)=>{let error=e.to_string();if error!=alignment_error {
                                crate::diagnostic_log!("Alignment component unavailable: {error}");alignment_error=error;
                            }},
                        }
                    }
                }
                // Auth readiness alone is not proof of a live connection to Home.
                let recent = rx.borrow().is_some_and(|t| t.elapsed() < Duration::from_secs(12));
                let connected = recent && peer.known_peers().snapshot().peers().iter().any(|p| p.peer_id() == expected);
                println!("{}", if connected { "connected" } else { "disconnected" });
            }
        }
    }
    println!("disconnected");
    requests.abort();
    if let Some((task, helper)) = drive_task {
        task.abort();
        let _ = task.await;
        drop(helper);
    }
    drop(alignment);
    if let Some(task) = home_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(task) = camera_task {
        task.abort();
        let _ = task.await;
    }
    drop(camera_publisher);
    if let Some(task)=depth_task { task.abort(); let _=task.await; }
    if let Some(d)=depth_publisher {d.close(publisher.as_ref().unwrap())?;}
    if let Some(task)=body_task { task.abort(); let _=task.await; }
    if let Some(b)=body_publisher {b.close(publisher.as_ref().unwrap())?;}
    let _ = requests.await;
    if let Some(publisher) = publisher {
        publisher.endpoint.close().await?;
        publisher.registry.close().await?;
    }
    let result = peer.shutdown().await;
    bootstrap.session().close().await;
    result?;
    if let Some(failure) = failure {
        anyhow::bail!(failure);
    }
    Ok(())
}
