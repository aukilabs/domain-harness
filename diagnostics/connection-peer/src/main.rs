//! Connection-only diagnostic; --poses adds exactly one typed BBOS pose Product.
mod alignment;
mod camera;
mod depth;
#[path = "../../../src/depth_points.rs"]
mod depth_points;
#[path = "../../../src/compact_pose.rs"]
mod compact_pose;
#[path = "../../../src/fisheye.rs"]
mod fisheye;
mod home;
mod poses;
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
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4
            || (args.len() == 5
                && matches!(args[4].as_str(), "--poses" | "--portals" | "--points" | "--check-home")),
        "usage: auki-connection-diagnostic PEER_CONFIG HOME_CONNECTION DIAGNOSTIC_IDENTITY [--poses|--portals|--points|--check-home]"
    );
    let check_home = args.get(4).is_some_and(|v| v == "--check-home");
    let publish_poses = args.len() == 5 && !check_home;
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
    eprintln!(
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
    let mut input = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let (camera_tx, mut camera_rx) =
        tokio::sync::watch::channel(None::<(camera::Calibration, u64, Vec<u8>)>);
    let camera_task = portals.then(|| {
        tokio::spawn(async move {
            use base64::Engine;
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
                let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
                let first = tokio::time::timeout(Duration::from_secs(30), lines.next_line())
                    .await??
                    .context("Camera configuration missing")?;
                let calibration: camera::Calibration = serde_json::from_str(&first)?;
                calibration.validate()?;
                #[derive(Deserialize)]
                struct Frame {
                    timestamp_ns: u64,
                    jpeg: String,
                }
                loop {
                    let line = lines.next_line().await?.context("Camera capture ended")?;
                    ensure!(line.len() < 3 * 1024 * 1024, "Camera IPC frame too large");
                    let frame: Frame = serde_json::from_str(&line)?;
                    let bytes = base64::engine::general_purpose::STANDARD.decode(frame.jpeg)?;
                    camera_tx.send_replace(Some((calibration.clone(), frame.timestamp_ns, bytes)));
                }
                #[allow(unreachable_code)]
                Ok::<(), anyhow::Error>(())
            };
            if let Err(e) = work.await {
                eprintln!("Camera capture failed: {e}");
            }
        })
    });
    let mut camera_publisher: Option<camera::Camera> = None;
    let mut camera_open = portals;
    let (depth_task, mut depth_rx) = if points {
        let (task,rx)=depth::start(); (Some(task),rx)
    } else { (None,tokio::sync::watch::channel(None).1) };
    let mut depth_open=points;
    let mut depth_publisher:Option<depth::Depth>=None;
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
                    eprintln!("Home alignment input unresolved: {error}");
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
    let client = ComponentProtocolClient::new(peer.protocols());
    let (tx, rx) = tokio::sync::watch::channel(None::<Instant>);
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
    loop {
        tokio::select! {
            changed=depth_rx.changed(),if depth_open => {
                if changed.is_err() {
                    depth_open=false;
                    if let Some(d)=depth_publisher.take(){d.close(publisher.as_ref().unwrap())?;}
                    eprintln!("Depth unavailable; pose and camera diagnostic continue");
                    continue;
                }
                let sample=depth_rx.borrow_and_update().clone();
                if let Some((config,sample))=sample {
                    let result=(|| {
                        if depth_publisher.is_none(){depth_publisher=Some(depth::Depth::mount(publisher.as_ref().unwrap(),config)?);}
                        depth_publisher.as_mut().unwrap().publish(sample)
                    })();
                    if let Err(e)=result {eprintln!("Depth publish failed: {e}");}
                }
            },
            changed=camera_rx.changed(),if camera_open => {
                if changed.is_err(){camera_open=false;alignment=None;camera_publisher=None;eprintln!("Camera unavailable; pose diagnostic continues");continue;}
                let sample=camera_rx.borrow_and_update().clone();
                if let Some((calibration,t,jpeg))=sample {
                    if camera_publisher.is_none(){
                        match camera::Camera::mount(publisher.as_ref().unwrap(),calibration) {
                            Ok(camera)=>camera_publisher=Some(camera),
                            Err(error)=>{camera_open=false;eprintln!("Camera unavailable: {error}; pose diagnostic continues");continue;}
                        }
                    }
                    if let Err(e)=camera_publisher.as_mut().unwrap().publish(t,jpeg){eprintln!("Camera publish failed: {e}");}
                }
            },
            line = input.next_line(), if publish_poses => {
                match line {
                    Ok(Some(line)) if line.len() <= 4096 => {
                        match serde_json::from_str::<poses::Sample>(&line).map_err(anyhow::Error::from)
                            .and_then(|s| publisher.as_mut().unwrap().publish(s)) {
                            Ok(()) => {}, Err(error) => { eprintln!("Pose input failed: {error}"); break; }
                        }
                    },
                    _ => { eprintln!("Pose input ended or invalid; stopping diagnostic"); break; }
                }
            },
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
            _ = peer.wait_stopped() => break,
            _ = tick.tick() => {
                if let (Some(publisher),Some(camera),Some(home))=(&publisher,&camera_publisher,home_rx.borrow().clone()) {
                    if alignment.as_ref().is_some_and(|a|a.target!=(&home.frame).into()) {alignment=None;}
                    if alignment.is_none() {
                        match alignment::Worker::mount(publisher,camera,home,home_rx.clone()) {
                            Ok(worker)=>alignment=Some(worker),Err(e)=>eprintln!("Alignment component unavailable: {e}"),
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
    let _ = requests.await;
    if let Some(publisher) = publisher {
        publisher.endpoint.close().await?;
        publisher.registry.close().await?;
    }
    let result = peer.shutdown().await;
    bootstrap.session().close().await;
    result?;
    Ok(())
}
