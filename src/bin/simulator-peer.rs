//! Authenticated peer sidecar. Godot's local bridge is NOT a network protocol.
use anyhow::{Result, ensure};
use spatial_home::{
    peer::PeerSettings,
    simulated_camera::{Camera, MAX_JPEG},
    simulated_lidar::{Lidar, MAX_BYTES, Scan},
};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

async fn capture(stream: &mut TcpStream, camera: &mut Camera) -> Result<()> {
    stream.write_all(&[1]).await?;
    let size = stream.read_u32_le().await? as usize;
    ensure!(
        size > 0 && size <= MAX_JPEG,
        "Godot image exceeds local bridge limit"
    );
    let timestamp = stream.read_u64_le().await?;
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).await?;
    // A viewer may close while rendering. Do not publish an unwanted frame.
    if camera.demanded() {
        camera.publish(timestamp, bytes)?;
    }
    Ok(())
}

async fn capture_lidar(stream: &mut TcpStream, lidar: &mut Lidar) -> Result<()> {
    stream.write_all(&[2]).await?;
    let size = stream.read_u32_le().await? as usize;
    ensure!(
        size > 0 && size <= MAX_BYTES,
        "Godot LiDAR scan exceeds bridge limit"
    );
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).await?;
    lidar.publish(serde_json::from_slice::<Scan>(&bytes)?)
}

// A hidden/minimized simulator may suspend rendering. Keep the in-flight
// response intact (including partial reads), and keep serving retained maps.
// Cancelling it on timeout would desynchronize this sequential bridge.
async fn await_capture(work: impl std::future::Future<Output = Result<()>>) -> Result<bool> {
    tokio::pin!(work);
    tokio::select! {
        result = &mut work => return result.map(|_| true),
        _ = tokio::signal::ctrl_c() => return Ok(false),
        _ = tokio::time::sleep(Duration::from_secs(2)) => eprintln!("Simulator capture delayed; awaiting the same response. Retained map remains available."),
    }
    tokio::select! {
        result = &mut work => result.map(|_| true),
        _ = tokio::signal::ctrl_c() => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn delayed_capture_keeps_the_original_request_alive() {
        let mut completed = false;
        assert!(super::await_capture(async {
            tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
            completed = true;
            Ok(())
        }).await.unwrap());
        assert!(completed);
        assert!(super::await_capture(async { anyhow::bail!("bridge closed") }).await.is_err());
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "data/simulator-peer.json".into());
    let settings = PeerSettings::load(Path::new(&config))?;
    // Bind before authentication, so a second instance cannot take the identity.
    let listener = TcpListener::bind("127.0.0.1:4331").await?;
    let running = settings.start().await?;
    let result = async {
        // Don't advertise a camera before its world connects.
        println!("Simulator peer {}: waiting for Godot on loopback:4331", running.peer.peer_id());
        let (mut stream, _) = tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            result = listener.accept() => result?,
        };
        stream.set_nodelay(true)?;
        let mut camera = Camera::mount(&running.peer)?;
        let mut lidar = Lidar::mount(&camera.runtime, &camera.endpoint)?;
        let mut mapper = spatial_home::simulator_mapper::Mapper::mount(&camera.runtime, &camera.endpoint, &lidar)?;
        let mut occupancy = None::<spatial_home::occupancy_mapper::OccupancyMapper>;
        println!("Camera catalog mounted: simulator-camera (640x480, demand-driven, max 10 FPS)");
        let work = async {
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut log = tokio::time::interval(Duration::from_secs(5));
            let mut captured = 0u64;
            let mut scans = 0u64;
            let mut map_tick = tokio::time::interval(Duration::from_secs(1));
            map_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => break,
                    _ = map_tick.tick() => {
                        if mapper.tick()? {
                            let product = mapper.product();
                            if occupancy.is_none() {
                                occupancy = Some(spatial_home::occupancy_mapper::OccupancyMapper::mount(
                                    &camera.runtime, &camera.endpoint, &product,
                                    camera.runtime.peer_id(), product.producer.clock.clone(),
                                    spatial_home::occupancy_mapper::Config::new(mapper.floor_m().expect("Mapped boot frame")),
                                )?);
                                println!("Robot occupancy mapper mounted: occupancy-2d (1 Hz, robot-local voxels)");
                            }
                            println!("Local mapper: {} scans integrated", product.latest_existing()?.unwrap().payload.voxels.integrated_observations);
                        }
                        if let Some(occupancy) = &mut occupancy { occupancy.tick()?; }
                    },
                    exit = running.peer.wait_stopped() => anyhow::bail!("Simulator transport stopped: {exit:?}"),
                    _ = log.tick() => println!("Camera: subscribers={}, frames={captured}; LiDAR: subscribers={}, scans={scans}", camera.endpoint.product_active_subscriptions(spatial_home::simulated_camera::PRODUCT), camera.endpoint.product_active_subscriptions(spatial_home::simulated_lidar::PRODUCT)),
                    _ = tick.tick() => {
                        if camera.demanded() {
                            if !await_capture(capture(&mut stream, &mut camera)).await? { break; }
                            captured += 1;
                        }
                        if lidar.demanded(&camera.endpoint) {
                            if !await_capture(capture_lidar(&mut stream, &mut lidar)).await? { break; }
                            scans += 1;
                        }
                        if !camera.demanded() && !lidar.demanded(&camera.endpoint) {
                            // Detect Godot exit even while no consumer is watching.
                            match stream.try_read(&mut [0u8; 1]) {
                                Ok(0) => break,
                                Ok(_) => anyhow::bail!("Unexpected Godot bridge data"),
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {},
                                Err(e) => return Err(e.into()),
                            }
                        }
                    }
                }
            }
            Ok(())
        }.await;
        drop(occupancy);
        drop(mapper);
        lidar.close();
        let close = camera.close().await;
        work.and(close)
    }.await;
    let shutdown = running.shutdown().await;
    result.and(shutdown)
}
