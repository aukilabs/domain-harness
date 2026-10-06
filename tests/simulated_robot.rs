use anyhow::Result;
use spatial_home::{
    model,
    occupancy_fixture::FakeVoxelSource,
    simulated_robot::{self, SimulationState},
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn single_robot_has_exactly_one_remote_peer_and_contributor() -> Result<()> {
    let (tx, mut rx) = tokio::sync::watch::channel(SimulationState::default());
    let stop = CancellationToken::new();
    let task = tokio::spawn(simulated_robot::run_counted(
        tx,
        stop.clone(),
        CancellationToken::new(),
        1,
    ));
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            rx.changed().await?;
            let state = rx.borrow().clone();
            anyhow::ensure!(state.status != "failed", "{}", state.detail);
            if state
                .occupancy_snapshot
                .as_ref()
                .is_some_and(|s| s.status == "ready")
            {
                assert_eq!(state.network["peers"].as_array().unwrap().len(), 1);
                let source = state.source_snapshot.unwrap();
                assert_eq!(
                    source["registries"]["contributors"]
                        .as_array()
                        .unwrap()
                        .len(),
                    1
                );
                assert_eq!(
                    source["registries"]["local_mapping"]["runners"]
                        .as_object()
                        .unwrap()
                        .len(),
                    1
                );
                break;
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(5), task).await???;
    result??;
    Ok(())
}

#[tokio::test]
async fn patrol_has_limited_visibility_and_clears_moving_obstacles() -> Result<()> {
    let runtime = auki_components::ComponentRuntime::new("robot");
    let mut source = FakeVoxelSource::new(&runtime, "robot", model::clock_entry("robot", "test"))?;
    let a = source.publish_patrol(0.)?;
    let b = source.publish_patrol(3.)?;
    a.validate("robot")?;
    b.validate("robot")?;
    assert_ne!(
        a.registries["local_mapping"]["robot_pose"]["pose"],
        b.registries["local_mapping"]["robot_pose"]["pose"]
    );
    let a_view = spatial_home::scene_mirror::render_scene(&a)?;
    let b_view = spatial_home::scene_mirror::render_scene(&b)?;
    assert_ne!(b_view["voxels"], a_view["voxels"]);
    let sample = |scene: &spatial_home::network::CanonicalScene, x: i32, z: i32| -> Result<i8> {
        let grid = spatial_home::occupancy_mapper::project(&scene.voxels, 0.)?;
        Ok(grid
            .cells
            .iter()
            .find(|c| c.index == [x.div_euclid(20), z.div_euclid(20)])
            .map(|c| c.values[(z.rem_euclid(20) * 20 + x.rem_euclid(20)) as usize])
            .unwrap_or(-1))
    };
    assert_eq!(sample(&a, 12, 0)?, 100);
    assert_eq!(
        sample(&b, 12, 0)?,
        0,
        "old obstacle position must become observed free"
    );
    assert_eq!(sample(&a, 12, 6)?, 0);
    assert_eq!(
        sample(&b, 12, 6)?,
        100,
        "new obstacle position must become occupied"
    );
    let opposite = source.publish_patrol(30.)?;
    assert_eq!(
        sample(&opposite, 12, 0)?,
        -1,
        "out of view must be unknown, not free or cached occupied"
    );
    let returned = source.publish_patrol(60.)?;
    assert_eq!(
        sample(&returned, 12, 0)?,
        100,
        "behavior must repeat after a full patrol"
    );
    assert!(source.publish_patrol(f64::NAN).is_err());
    Ok(())
}

#[tokio::test]
async fn occupancy_grows_retains_unseen_obstacles_and_clears_only_with_free_evidence() -> Result<()>
{
    use spatial_home::occupancy_mapper::{Config, OccupancyMapper};
    let runtime = auki_components::ComponentRuntime::new("robot");
    let clock = model::clock_entry("robot", "accumulation-test");
    let mut source = FakeVoxelSource::new(&runtime, "robot", clock.clone())?;
    let mut config = Config::new(0.);
    config.interval = Duration::from_millis(100);
    let mut mapper = OccupancyMapper::mount_local(
        &runtime,
        &source.product(),
        "robot",
        model::clock_ref(&clock),
        config,
    )?;
    let mut count = 0;
    for (seconds, expected) in [(0., 100), (30., 100), (3., 0)] {
        source.publish_patrol(seconds)?;
        tokio::time::sleep(Duration::from_millis(150)).await;
        mapper.tick()?;
        let grid = mapper.state().layers.occupancy_2d.as_ref().unwrap();
        let observed = grid
            .cells
            .iter()
            .flat_map(|c| &c.values)
            .filter(|v| **v != -1)
            .count();
        assert!(observed >= count, "known area must never shrink");
        if seconds == 30. {
            assert!(
                observed > count,
                "exploring the opposite side must grow the map"
            );
        }
        count = observed;
        let cell = grid.cells.iter().find(|c| c.index == [0, 0]).unwrap();
        assert_eq!(
            cell.values[12], expected,
            "obstacle at X=1.25 Z=.05 at time {seconds}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn loopback_robot_drives_home_occupancy_and_stops_cleanly() -> Result<()> {
    let (tx, mut rx) = tokio::sync::watch::channel(SimulationState::default());
    let stop = CancellationToken::new();
    let disconnect = CancellationToken::new();
    let task = tokio::spawn(simulated_robot::run_controlled(
        tx,
        stop.clone(),
        disconnect.clone(),
    ));
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let mut first_pose = None;
        let mut first_grid = None;
        loop {
            rx.changed().await?;
            let s = rx.borrow().clone();
            anyhow::ensure!(s.status != "failed", "{}", s.detail);
            if let (Some(source), Some(occupancy), Some(view)) =
                (&s.source_snapshot, &s.occupancy_snapshot, &s.occupancy)
            {
                if occupancy.status != "ready" {
                    continue;
                }
                assert_eq!(source["authority_peer_id"], occupancy.authority_peer_id);
                let contributors = source["registries"]["contributors"].as_array().unwrap();
                let devices = s.network["peers"].as_array().unwrap();
                assert_eq!(devices.len(), 2);
                assert_eq!(
                    s.network["connection"]["peer_id"],
                    source["authority_peer_id"]
                );
                for device in devices {
                    assert!(contributors.contains(&device["peer_id"]));
                    assert!(device["received_observations"].as_u64().unwrap() > 0);
                    assert!(
                        device["catalog"]["components"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|c| c["manifest"]["component_id"] == "fake-voxel-source")
                    );
                    let products = device["catalog"]["products"].as_array().unwrap();
                    assert!(
                        products
                            .iter()
                            .any(|p| p["manifest"]["product_id"] == "fake-voxels-scene"
                                && p["manifest"]["peer_id"] == device["peer_id"])
                    );
                }
                assert_eq!(contributors.len(), 2);
                assert_ne!(contributors[0], contributors[1]);
                for robot in contributors {
                    assert_ne!(robot, &source["authority_peer_id"]);
                }
                assert_eq!(
                    view["local_mapping"]["runners"].as_object().unwrap().len(),
                    2
                );
                assert_eq!(
                    occupancy.source_product.peer_id,
                    source["authority_peer_id"].as_str().unwrap()
                );
                let grid = occupancy.layers.occupancy_2d.as_ref().unwrap();
                assert_eq!(grid.resolution_m, 0.1);
                assert_eq!(grid.cell_size_m, 2.);
                let values: Vec<_> = grid
                    .cells
                    .iter()
                    .flat_map(|c| c.values.iter().copied())
                    .collect();
                for expected in [-1, 0, 100] {
                    assert!(values.contains(&expected));
                }
                assert!(!view["occupancy_tiles"].as_array().unwrap().is_empty());
                assert!(view["voxels"].as_array().unwrap().is_empty());
                let pose = source["registries"]["local_mapping"]["robot_pose"]["pose"].clone();
                let grid_json = serde_json::to_value(grid)?;
                if first_pose.as_ref().is_some_and(|p| p != &pose)
                    && first_grid.as_ref().is_some_and(|g| g != &grid_json)
                {
                    break;
                }
                first_pose.get_or_insert(pose);
                first_grid.get_or_insert(grid_json);
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    if result.as_ref().is_ok_and(|r| r.is_ok()) {
        disconnect.cancel();
        let disconnected = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if rx.borrow().status == "disconnected" {
                    break;
                }
                rx.changed().await?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await;
        if disconnected.as_ref().is_ok_and(|r| r.is_ok()) {
            let retained = rx.borrow().clone();
            assert!(retained.voxels.is_none());
            assert!(retained.source_snapshot.is_none());
            assert!(retained.occupancy.is_some());
            assert!(
                retained
                    .occupancy_snapshot
                    .unwrap()
                    .layers
                    .occupancy_2d
                    .is_some()
            );
            assert!(
                retained.network["peers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|p| p["status"] == "disconnected")
            );
            assert!(
                !task.is_finished(),
                "Home must remain running after robot shutdown"
            );
        }
        stop.cancel();
        disconnected??;
    } else {
        stop.cancel();
    }
    tokio::time::timeout(Duration::from_secs(5), task).await???;
    result??;
    assert_eq!(rx.borrow().status, "stopped");
    Ok(())
}

#[tokio::test]
async fn two_sources_merge_unique_coverage_without_inventing_observations() -> Result<()> {
    let r1 = auki_components::ComponentRuntime::new("r1");
    let r2 = auki_components::ComponentRuntime::new("r2");
    let home = auki_components::ComponentRuntime::new("home");
    let mut a = FakeVoxelSource::new(&r1, "r1", model::clock_entry("r1", "test"))?;
    let mut b = FakeVoxelSource::new(&r2, "r2", model::clock_entry("r2", "test"))?;
    let mut combined = FakeVoxelSource::new(&home, "home", model::clock_entry("home", "test"))?;
    let a = a.publish_patrol(0.)?;
    let b0 = b.publish_patrol_phase(0., std::f64::consts::PI)?;
    let scene = combined.publish_combined(&[a.clone(), b0.clone()])?;
    let grid = spatial_home::occupancy_mapper::project(&scene.voxels, 0.)?;
    let sample = |x: i32, z: i32| {
        grid.cells
            .iter()
            .find(|c| c.index == [x.div_euclid(20), z.div_euclid(20)])
            .map(|c| c.values[(z.rem_euclid(20) * 20 + x.rem_euclid(20)) as usize])
            .unwrap_or(-1)
    };
    assert_eq!(sample(18, 0), 0, "robot 1 supplies right-side coverage");
    assert_eq!(sample(-18, 0), 0, "robot 2 supplies left-side coverage");
    assert_eq!(sample(0, 0), -1, "unseen middle must remain unknown");
    let observed = |g: &spatial_home::occupancy_mapper::OccupancyGrid| {
        g.cells
            .iter()
            .flat_map(|c| &c.values)
            .filter(|v| **v != -1)
            .count()
    };
    for source in [&a, &b0] {
        assert!(
            observed(&grid)
                > observed(&spatial_home::occupancy_mapper::project(
                    &source.voxels,
                    0.
                )?)
        );
    }
    let later = b.publish_patrol_phase(1., std::f64::consts::PI)?;
    assert!(
        combined.publish_combined(&[a, later]).is_err(),
        "cannot merge different world times"
    );
    Ok(())
}
