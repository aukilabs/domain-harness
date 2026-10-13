//! Connection-only diagnostic; --poses adds exactly one typed BBOS pose Product.
mod poses;
#[path = "../../../src/compact_pose.rs"]
mod compact_pose;
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
#[derive(Deserialize)]
struct Home {
    schema: String,
    domain_id: String,
    peer_id: String,
    routes: Vec<String>,
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
        args.len() == 4 || (args.len() == 5 && args[4] == "--poses"),
        "usage: auki-connection-diagnostic PEER_CONFIG HOME_CONNECTION DIAGNOSTIC_IDENTITY [--poses]"
    );
    let publish_poses = args.len() == 5;
    let config: Config = read(&args[1])?;
    let home: Home = read(&args[2])?;
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
    let mut publisher = if publish_poses {
        Some(poses::Publisher::mount(&peer)?)
    } else {
        None
    };
    let mut input = tokio::io::BufReader::new(tokio::io::stdin()).lines();
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
                // Auth readiness alone is not proof of a live connection to Home.
                let recent = rx.borrow().is_some_and(|t| t.elapsed() < Duration::from_secs(12));
                let connected = recent && peer.known_peers().snapshot().peers().iter().any(|p| p.peer_id() == expected);
                println!("{}", if connected { "connected" } else { "disconnected" });
            }
        }
    }
    println!("disconnected");
    requests.abort();
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
