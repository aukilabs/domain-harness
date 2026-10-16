mod support;
fn occupied(
    snapshot: &auki_voxel_map::VoxelSnapshot,
) -> anyhow::Result<Vec<spatial_home::raycast::Voxel>> {
    Ok(snapshot
        .accumulator()?
        .viewer_snapshot(0.)?
        .chunks
        .into_iter()
        .flat_map(|c| c.voxels)
        .filter(|v| v.occupancy_evidence > 0.)
        .map(|v| spatial_home::raycast::Voxel {
            center: v.center_m,
            evidence: v.occupancy_evidence,
            linear_rgb: v.linear_rgb,
        })
        .collect())
}
use anyhow::Result;
use auki_component_protocol::ComponentProtocolClient;
use spatial_home::{
    device_catalogs::{Catalogs, fetch},
    sensor_preview::{read_scene, scene_products},
    simulated_camera::Camera,
    simulated_lidar::{Lidar, Scan},
    simulator_mapper::Mapper,
};

fn scan(timestamp_ns: u64, x: f32, distance: Option<f32>) -> Scan {
    let mut ranges_m = vec![None; 1936];
    ranges_m[7 * 121 + 60] = distance;
    Scan {
        timestamp_ns,
        ranges_m,
        world_position: [x, 0.65, -0.28],
        world_rotation_xyzw: [0., 0., 0., 1.],
    }
}
async fn integrate(mapper: &mut Mapper) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !mapper.tick()? {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn robot_runs_both_mappers_and_exports_discoverable_occupancy() -> Result<()> {
    use spatial_home::occupancy_mapper::{Config, OccupancyMapper, render_scene};
    let pair = support::LocalPair::start().await?;
    let camera = Camera::mount(&pair.b)?;
    let mut lidar = Lidar::mount(&camera.runtime, &camera.endpoint, &camera.session_clock)?;
    let mut mapper = Mapper::mount(&camera.runtime, &camera.endpoint, &lidar, camera.session_clock.clone())?;
    let mut first = scan(1, 0., Some(2.));
    first.world_position[1] = 0.85; // Boot body 20 cm above world floor.
    lidar.publish(first)?;
    integrate(&mut mapper).await?;
    assert!((mapper.floor_m().unwrap() + 0.2).abs() < 1e-6);
    let product = mapper.product();
    let mut occupancy = OccupancyMapper::mount(
        &camera.runtime,
        &camera.endpoint,
        &product,
        camera.runtime.peer_id(),
        camera.session_clock.clone(),
        Config::new(mapper.floor_m().unwrap()),
    )?;
    assert_eq!(occupancy.interval(), std::time::Duration::from_secs(1));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            occupancy.tick()?;
            if occupancy.state().status == "ready" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    let scene = occupancy.state().clone();
    assert_eq!(scene.source_product, product.reference());
    assert_eq!(scene.authority_peer_id, pair.b.peer_id().to_string());
    let view = render_scene(&scene, &scene.authority_peer_id)?;
    assert_eq!(view["occupancy_counts"]["occupied"], 1);
    assert_eq!(
        view["occupancy_counts"]["free"], 0,
        "Sparse LiDAR must not invent fully observed free columns"
    );
    let mut malformed = scene.clone();
    malformed.layers.occupancy_2d.as_mut().unwrap().up_axis = 99;
    assert!(render_scene(&malformed, &scene.authority_peer_id).is_err());
    assert!(render_scene(&scene, "wrong-owner").is_err());
    let routes = pair
        .b
        .listen_addresses()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let catalogs = Catalogs::from([(
        pair.b.peer_id().to_string(),
        fetch(&pair.a, pair.b.peer_id(), &routes).await,
    )]);
    let scenes = scene_products(&catalogs);
    assert_eq!(scenes.len(), 2);
    let client = ComponentProtocolClient::new(pair.a.protocols());
    let mut found = false;
    for source in &scenes {
        let remote = read_scene(&client, source).await?;
        if remote["product_id"] == "occupancy-2d" {
            assert_eq!(remote["layer_type"], "occupancy_2d");
            assert_eq!(remote["occupancy_counts"]["occupied"], 1);
            found = true;
        }
    }
    assert!(found);
    assert!(!camera.demanded());
    drop(occupancy);
    drop(mapper);
    lidar.close();
    camera.close().await?;
    pair.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_mapping_and_discovered_scene_round_trip() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let camera = Camera::mount(&pair.b)?;
    let mut lidar = Lidar::mount(&camera.runtime, &camera.endpoint, &camera.session_clock)?;
    let mut mapper = Mapper::mount(&camera.runtime, &camera.endpoint, &lidar, camera.session_clock.clone())?;
    assert!(
        lidar.demanded(&camera.endpoint),
        "Local binding must create demand"
    );
    assert!(!camera.demanded());
    assert!(!mapper.tick()?);
    lidar.publish(scan(1, 0., Some(2.)))?;
    integrate(&mut mapper).await?;
    let first = mapper.product().latest_existing()?.unwrap();
    first.payload.validate(&pair.b.peer_id().to_string())?;
    let initial = occupied(&first.payload.voxels)?;
    assert_eq!(initial.len(), 1);
    assert!(first.payload.frame.starts_with("simulator-local:"));
    assert!(
        first
            .payload
            .portals
            .scenegraph
            .map
            .domain_reference
            .is_none()
    );
    assert!(!mapper.tick()?, "Do not reintegrate a scan");
    lidar.publish(scan(2, 2., Some(2.)))?;
    integrate(&mut mapper).await?;
    let next = mapper.product().latest_existing()?.unwrap();
    assert_eq!(
        occupied(&next.payload.voxels)?.len(),
        2,
        "Retain unseen cells as robot moves"
    );
    lidar.publish(scan(3, 2., None))?;
    integrate(&mut mapper).await?;
    assert_eq!(
        occupied(&mapper.product().latest_existing()?.unwrap().payload.voxels)?.len(),
        2,
        "No return must not erase the map"
    );
    // Repeated measured rays through the original endpoint clear it.
    for t in 4..7 {
        lidar.publish(scan(t, 0., Some(4.)))?;
        integrate(&mut mapper).await?;
    }
    let cleared = occupied(&mapper.product().latest_existing()?.unwrap().payload.voxels)?;
    assert!(!cleared.iter().any(|v| v.center == initial[0].center));
    let routes = pair
        .b
        .listen_addresses()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let row = fetch(&pair.a, pair.b.peer_id(), &routes).await;
    let mut catalogs = Catalogs::from([(pair.b.peer_id().to_string(), row)]);
    let scenes = scene_products(&catalogs);
    assert_eq!(scenes.len(), 1);
    let view = read_scene(
        &ComponentProtocolClient::new(pair.a.protocols()),
        &scenes[0],
    )
    .await?;
    assert_eq!(view["product_id"], "simulator-local-scene");
    assert_eq!(view["voxels"].as_array().unwrap().len(), 2);
    assert!(
        !camera.demanded(),
        "Reading maps must not activate the camera"
    );
    let mut full = scan(7, 0., None);
    full.ranges_m.fill(Some(10.));
    lidar.publish(full)?;
    integrate(&mut mapper).await?;
    mapper
        .product()
        .latest_existing()?
        .unwrap()
        .payload
        .validate(&pair.b.peer_id().to_string())?;
    let row = catalogs.values_mut().next().unwrap();
    row["catalog_status"] = "unavailable".into();
    assert!(scene_products(&catalogs).is_empty());
    let row = catalogs.values_mut().next().unwrap();
    row["catalog_status"] = "ready".into();
    for product in row["catalog"]["products"].as_array_mut().unwrap() {
        product["manifest"]["peer_id"] = "wrong-owner".into();
    }
    assert!(scene_products(&catalogs).is_empty());
    drop(mapper);
    assert!(
        !lidar.demanded(&camera.endpoint),
        "Dropping local binding must release demand"
    );
    lidar.close();
    camera.close().await?;
    pair.shutdown().await?;
    Ok(())
}
