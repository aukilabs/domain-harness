//! Read-only scene consumer. This binary has no robot motion/task adapter.
use anyhow::{Result, bail, ensure};
use auki_component_protocol::RemoteObservationEvent;
use spatial_home::{
    model::clock_ref,
    network::subscribe_scene,
    peer::{PeerSettings, read_connection},
};
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut config = None;
    let mut home = None;
    let mut output = None;
    let mut once = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config = args.next().map(PathBuf::from),
            "--home" => home = args.next().map(PathBuf::from),
            "--output" => output = args.next().map(PathBuf::from),
            "--once" => once = true,
            "--help" | "-h" => {
                println!(
                    "spatial-runner --config PEER.json --home HOME-CONNECTION.json [--once] [--output SCENE.json]\nConsumes canonical scene snapshots only; no motion or task execution.\nStops on transport failure; restart to rediscover the current Product."
                );
                return Ok(());
            }
            _ => bail!("unknown option {arg}; use --help"),
        }
    }
    let settings =
        PeerSettings::load(&config.ok_or_else(|| anyhow::anyhow!("--config required"))?)?;
    let home = read_connection(&home.ok_or_else(|| anyhow::anyhow!("--home required"))?)?;
    let running = settings.start().await?;
    let work = async {
        let runtime = auki_components::ComponentRuntime::new(running.peer.peer_id().to_string());
        let endpoint = auki_component_protocol::ComponentProtocolEndpoint::mount(running.peer.protocols(), runtime.clone())?;
        let clock = auki_time::SessionClock::new(runtime.peer_id(), uuid::Uuid::new_v4().to_string(), "session");
        let mut time = spatial_home::capture_time::Publisher::mount(&runtime, &endpoint, clock.registry_entry())?;
        time.sample(&spatial_home::capture_time::SessionUtcClock(clock.clone()))?;
        let definitions = spatial_home::product_registry::Definitions::default();
        spatial_home::product_registry::clock(&definitions, &clock.registry_entry())?;
        spatial_home::product_registry::clock(&definitions, &time.contract.to_clock)?;
        let registry = spatial_home::product_registry::mount(&running.peer, runtime, definitions)?;
        let work = async {
        let mut subscription = subscribe_scene(&running.peer, &home).await?;
        let source_clock = subscription.product().producer.clock.clone();
        println!("Subscribed to Home {} in {}. Robot alignment is not established by this connection.", home.peer_id, home.frame);
        let consume = async {
            let mut first = true;
            loop {
                let event = if first {
                    tokio::time::timeout(Duration::from_secs(30), subscription.next()).await??
                } else { subscription.next().await? };
                match event {
                    Some(RemoteObservationEvent::Observation(observation)) => {
                        let scene = &observation.payload;
                        scene.validate(&home.peer_id)?;
                        ensure!(clock_ref(&scene.publication_clock) == source_clock, "source publication clock mismatch");
                        if let Some(path) = &output {
                            write_snapshot(path, scene)?;
                        }
                        println!("Scene: {} Portals, {} integrated observations, {} semantic objects; frame {}",
                            scene.portals.scenegraph.anchors.len(), scene.voxels.integrated_observations, scene.objects.len(), scene.frame);
                        first = false;
                        if once { return Ok::<_, anyhow::Error>(()); }
                    },
                    Some(RemoteObservationEvent::Gap(gap)) => eprintln!("Retention gap: {gap:?}; next complete snapshot replaces local state"),
                    Some(RemoteObservationEvent::Closed(reason)) => bail!("Home publication closed: {reason:?}"),
                    None => bail!("Home subscription ended"),
                }
            }
        };
        let result = tokio::select! {
            result = consume => result,
            exit = running.peer.wait_stopped() => Err(anyhow::anyhow!("runner peer stopped: {exit:?}")),
            _ = tokio::signal::ctrl_c() => Ok(()),
        };
        let cleanup = subscription.close().await;
        result?;
        cleanup?;
        Ok::<_, anyhow::Error>(())
        }.await;
        let close_registry = registry.close().await;
        let close_endpoint = endpoint.close().await;
        work.and(close_registry.map_err(Into::into)).and(close_endpoint.map_err(Into::into))
    }.await;
    let cleanup = running.shutdown().await;
    work.and(cleanup)
}

fn write_snapshot(path: &std::path::Path, value: &impl serde::Serialize) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".scene-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        serde_json::to_writer(&mut file, value)?;
        file.flush()?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
