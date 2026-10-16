use anyhow::Result;
use auki_components::ComponentRuntime;
use spatial_home::{
    occupancy_fixture::{FakeVoxelSource, Scenario},
    occupancy_mapper::{Config, FREE, OCCUPIED, OccupancyMapper},
};
use std::time::Duration;

async fn wait_center(mapper: &mut OccupancyMapper, expected: i8) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            mapper.tick()?;
            let value = mapper
                .state()
                .layers
                .occupancy_2d
                .as_ref()
                .and_then(|g| g.cells.iter().find(|c| c.index == [0, 0]))
                .map(|c| c.values[0]);
            if mapper.state().status == "ready" && value == Some(expected) {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn synthetic_component_drives_real_mapper_without_peer_or_credentials() -> Result<()> {
    let runtime = ComponentRuntime::new("fixture-test");
    let clock = auki_time::SessionClock::new("fixture-test", "test-session", "session");
    let mut source = FakeVoxelSource::new(&runtime, "fixture-test", clock.clone())?;
    let mut config = Config::new(0.);
    config.interval = Duration::from_millis(100);
    config.stale_after = Duration::from_millis(300);
    let mut mapper = OccupancyMapper::mount_local(
        &runtime,
        &source.product(),
        "fixture-test",
        clock,
        config,
    )?;
    source.publish(Scenario::Obstacle)?;
    wait_center(&mut mapper, OCCUPIED).await?;
    source.publish(Scenario::Clear)?;
    wait_center(&mut mapper, FREE).await?;
    let scene = source.publish(Scenario::Overhead)?;
    assert!(scene.registries["simulation"].as_bool().unwrap());
    // Let the new input traverse the SDK binding before checking the next cycle.
    tokio::time::sleep(Duration::from_millis(150)).await;
    wait_center(&mut mapper, FREE).await?;
    source.publish(Scenario::Unknown)?;
    wait_center(&mut mapper, FREE).await?; // Missing evidence preserves previously observed free space.
    tokio::time::sleep(Duration::from_millis(400)).await;
    mapper.tick()?;
    assert_eq!(mapper.state().status, "stale");
    source.publish(Scenario::Obstacle)?;
    wait_center(&mut mapper, OCCUPIED).await?;
    Ok(())
}
