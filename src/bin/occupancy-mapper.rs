//! Run the same mapper on any peer using an authenticated remote scene Product.
use anyhow::{Result, bail};
use auki_component_protocol::{ComponentProtocolEndpoint, RemoteObservationEvent};
use auki_components::ComponentRuntime;
use spatial_home::{
    model::{clock_entry, clock_ref},
    network::subscribe_scene_product,
    occupancy_mapper::{Config, OccupancyMapper},
    peer::{PeerSettings, read_connection},
};
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut config, mut source, mut floor) = (None, None, None);
    let mut product = "home-live-scene".to_owned();
    let mut hz = 1.0_f64;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config = args.next().map(PathBuf::from),
            "--source" => source = args.next().map(PathBuf::from),
            "--product" => {
                product = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--product needs a value"))?
            }
            "--floor-m" => {
                floor = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--floor-m needs a value"))?
                        .parse::<f64>()?,
                )
            }
            "--hz" => {
                hz = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--hz needs a value"))?
                    .parse()?
            }
            "--help" | "-h" => {
                println!(
                    "occupancy-mapper --config PEER.json --source CONNECTION.json --floor-m METERS [--product home-live-scene] [--hz 1]\nPublishes occupancy-2d on this peer. Uses source voxels at 10 cm resolution, flat floor, first meter above floor.\nStops on transport failure; restart to rediscover the current source Product. No robot commands."
                );
                return Ok(());
            }
            _ => bail!("unknown option {arg}"),
        }
    }
    anyhow::ensure!(
        hz.is_finite() && (1.0 / 60.0..=10.).contains(&hz),
        "--hz must be between 1/60 and 10"
    );
    let mut mapping = Config::new(floor.ok_or_else(|| {
        anyhow::anyhow!("--floor-m required; source origin is not necessarily the floor")
    })?);
    anyhow::ensure!(
        mapping.floor_m.is_finite() && mapping.floor_m.abs() < 1_000_000.,
        "invalid floor height"
    );
    mapping.interval = Duration::from_secs_f64(1. / hz);
    mapping.stale_after = Duration::from_secs(5).max(mapping.interval * 3);
    let settings =
        PeerSettings::load(&config.ok_or_else(|| anyhow::anyhow!("--config required"))?)?;
    let connection = read_connection(&source.ok_or_else(|| anyhow::anyhow!("--source required"))?)?;
    let running = settings.start().await?;
    let result = async {
        let mut subscription = subscribe_scene_product(&running.peer, &connection, &product).await?;
        let runtime = ComponentRuntime::new(running.peer.peer_id().to_string());
        let endpoint = ComponentProtocolEndpoint::mount(running.peer.protocols(), runtime.clone())?;
        let work = async {
            let owner = running.peer.peer_id().to_string();
            let clock = clock_entry(&owner, &uuid::Uuid::new_v4().to_string());
            let mut mapper = OccupancyMapper::mount(&runtime, &endpoint, subscription.product(), &owner, clock_ref(&clock), mapping)?;
            let mut tick = tokio::time::interval(mapper.interval());
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            println!("Publishing occupancy-2d on peer {owner}; source {} / {product}", connection.peer_id);
            loop {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => break,
                    exit = running.peer.wait_stopped() => bail!("peer stopped: {exit:?}"),
                    _ = tick.tick() => { mapper.tick()?; },
                    event = subscription.next() => match event? {
                        Some(RemoteObservationEvent::Observation(_)) | Some(RemoteObservationEvent::Gap(_)) => {},
                        _ => bail!("source Product disconnected; restart to reconnect"),
                    }
                }
            }
            Ok::<_, anyhow::Error>(())
        }.await;
        let close_input = subscription.close().await;
        let close_output = endpoint.close().await;
        work.and(close_input.map_err(Into::into)).and(close_output.map_err(Into::into))
    }.await;
    let cleanup = running.shutdown().await;
    result.and(cleanup)
}
