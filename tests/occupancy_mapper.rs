mod support;
use anyhow::Result;
use auki_components::*;
use auki_datatypes::map::{MapUpdate, VoxelChunkSnapshot, VoxelMapCheckpoint, VoxelSnapshot};
use prost::Message;
use spatial_home::{
    model::{self, Event},
    occupancy_mapper::*,
};
use std::{collections::BTreeMap, time::Duration};

fn built() -> Result<model::Built> {
    let fixtures = model::fixtures()?;
    let event = Event::Contribution(serde_json::from_value(
        fixtures["contributions"][0].clone(),
    )?);
    model::build(&[event])
}
fn set_points(snapshot: &mut auki_voxel_map::VoxelSnapshot, points: &[([i32; 3], f64)]) {
    let mut chunks = BTreeMap::<[i32; 3], Vec<VoxelSnapshot>>::new();
    for (p, evidence) in points {
        chunks
            .entry(p.map(|x| x.div_euclid(16)))
            .or_default()
            .push(VoxelSnapshot {
                x: p[0].rem_euclid(16) as u32,
                y: p[1].rem_euclid(16) as u32,
                z: p[2].rem_euclid(16) as u32,
                occupancy_evidence: *evidence,
                semantics: vec![],
                color: None,
            });
    }
    snapshot.checkpoint = MapUpdate {
        voxel_chunks: vec![],
        checkpoint: Some(VoxelMapCheckpoint {
            voxel_chunks: chunks
                .into_iter()
                .map(|(p, voxels)| VoxelChunkSnapshot {
                    chunk_x: p[0],
                    chunk_y: p[1],
                    chunk_z: p[2],
                    voxels,
                })
                .collect(),
        }),
    }
    .encode_to_vec();
}
fn value(grid: &OccupancyGrid, u: i32, v: i32) -> i8 {
    let width=cell_width(grid).unwrap() as i32;
    grid.cells
        .iter()
        .find(|c| c.index == [u.div_euclid(width), v.div_euclid(width)])
        .map_or(UNKNOWN, |c| {
            c.values[(v.rem_euclid(width) * width + u.rem_euclid(width)) as usize]
        })
}

#[test]
fn three_cm_input_requires_complete_free_coverage_and_preserves_two_metre_cells()->Result<()> {
    let mut snapshot=built()?.voxels;
    let auki_registry::MapBody::Voxel(grid)=&mut snapshot.definition.registry.body;
    grid.voxel_size_m.0=0.03;
    let up=[snapshot.definition.frame.axes.x,snapshot.definition.frame.axes.y,snapshot.definition.frame.axes.z]
        .iter().position(|a|*a==auki_registry::AxisDirection::Up).unwrap();
    let horizontal:Vec<_>=(0..3).filter(|i|*i!=up).collect();
    let point=|x,y,z| {let mut p=[0;3];p[horizontal[0]]=x;p[up]=y;p[horizontal[1]]=z;p};
    let mut points=Vec::new();
    for x in 0..4 {for z in 0..4 {for y in 0..33 {points.push((point(x,y,z),-1.));}}}
    set_points(&mut snapshot,&points);
    assert_eq!(value(&project(&snapshot,0.)?,0,0),FREE);
    points.pop();set_points(&mut snapshot,&points);
    assert_eq!(value(&project(&snapshot,0.)?,3,3),UNKNOWN);
    points.iter_mut().find(|(p,_)|*p==point(3,10,3)).unwrap().1=1.;set_points(&mut snapshot,&points);
    let grid=project(&snapshot,0.)?;
    assert_eq!(grid.resolution_m,0.03);
    assert_eq!(cell_width(&grid)?,67);
    assert_eq!(value(&grid,3,3),OCCUPIED);
    assert_eq!(value(&grid,0,0),FREE);
    set_points(&mut snapshot,&[(point(66,0,0),1.),(point(67,0,0),1.),(point(-1,0,0),1.)]);
    let layers=spatial_home::voxel_layers::split(&snapshot)?;
    assert_eq!(layers[0].resolution_m,0.03);assert_eq!(layers[0].cell_size_m,2.);
    let indices:Vec<_>=layers[0].cells.iter().map(|c|c.index[horizontal[0]]).collect();
    assert!(indices.contains(&-1)&&indices.contains(&0)&&indices.contains(&1));
    Ok(())
}

#[test]
fn full_capacity_projection() -> Result<()> {
    let mut snapshot = built()?.voxels;
    let points: Vec<_> = (0..65_536).map(|i| ([i % 256, 3, i / 256], 1.)).collect();
    set_points(&mut snapshot, &points);
    let started = std::time::Instant::now();
    let map = project(&snapshot, 0.)?;
    eprintln!(
        "65,536-voxel projection including validation: {:?}",
        started.elapsed()
    );
    assert_eq!(
        map.cells
            .iter()
            .flat_map(|c| &c.values)
            .filter(|v| **v == OCCUPIED)
            .count(),
        65_536
    );
    assert_eq!(Config::new(0.).interval, Duration::from_secs(1));
    Ok(())
}

#[test]
fn band_free_coverage_negative_cells_and_replacement() -> Result<()> {
    let mut snapshot = built()?.voxels;
    let mut points = vec![
        ([-1, 0, -21], 1.),
        ([20, 9, 0], 1.),
        ([0, -1, 0], 1.),
        ([1, 10, 0], 1.),
    ];
    points.extend((0..10).map(|y| ([2, y, 0], -1.)));
    points.extend((0..9).map(|y| ([3, y, 0], -1.)));
    points.extend((0..10).map(|y| ([4, y, 0], if y == 2 { 0. } else { -1. })));
    set_points(&mut snapshot, &points);
    let map = project(&snapshot, 0.)?;
    assert_eq!(map.horizontal_axes, [0, 2]);
    assert_eq!(value(&map, -1, -21), OCCUPIED);
    assert_eq!(value(&map, 20, 0), OCCUPIED);
    assert_eq!(value(&map, 0, 0), UNKNOWN); // floor below plane
    assert_eq!(value(&map, 1, 0), UNKNOWN); // ceiling above band
    assert_eq!(value(&map, 2, 0), FREE);
    assert_eq!(value(&map, 3, 0), UNKNOWN); // partial visibility
    assert_eq!(value(&map, 4, 0), UNKNOWN); // zero isn't free
    assert!(map.cells.iter().all(|c| c.values.len() == 400));
    // Replacing evidence clears occupied cells; missing evidence becomes unknown.
    set_points(
        &mut snapshot,
        &(0..10).map(|y| ([-1, y, -21], -1.)).collect::<Vec<_>>(),
    );
    let cleared = project(&snapshot, 0.)?;
    assert_eq!(value(&cleared, -1, -21), FREE);
    assert_eq!(value(&cleared, 20, 0), UNKNOWN);
    Ok(())
}

#[test]
fn translated_floor_and_z_up_and_invalid_input() -> Result<()> {
    let mut snapshot = built()?.voxels;
    set_points(
        &mut snapshot,
        &[
            ([0, 5, 0], 1.),
            ([1, 6, 0], 1.),
            ([2, 15, 0], 1.),
            ([3, 16, 0], 1.),
        ],
    );
    let map = project(&snapshot, 0.55)?;
    assert_eq!(value(&map, 0, 0), UNKNOWN); // center exactly on floor excluded
    assert_eq!(value(&map, 1, 0), OCCUPIED);
    assert_eq!(value(&map, 2, 0), OCCUPIED); // center exactly 1m above floor included
    assert_eq!(value(&map, 3, 0), UNKNOWN);
    let target = spatial_home::conventions::RUNNER_FRAME;
    let mut definition = snapshot.definition.clone();
    definition.frame = spatial_home::conventions::registry(&definition.frame.peer_id, target)?;
    definition.portal_map.frame = spatial_home::conventions::frame(target)?;
    let auki_registry::MapBody::Voxel(grid) = &mut definition.registry.body;
    grid.frame = auki_registry::RegistryRef {
        peer_id: definition.frame.peer_id.clone(),
        id: target.into(),
        hash: definition.frame.hash(),
    };
    let converted = spatial_home::conventions::voxel_checkpoint(&snapshot, definition)?;
    let z = project(&converted, 0.55)?;
    assert_eq!(z.up_axis, 2);
    assert_eq!(z.horizontal_axes, [0, 1]);
    assert_eq!(
        z.cells
            .iter()
            .flat_map(|c| &c.values)
            .filter(|v| **v == OCCUPIED)
            .count(),
        2
    );
    assert!(project(&snapshot, f64::NAN).is_err());
    snapshot.checkpoint = vec![255];
    assert!(project(&snapshot, 0.).is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn component_binds_remote_product_and_exports_current_occupancy() -> Result<()> {
    use auki_component_protocol::{
        ComponentProtocolClient, ComponentProtocolEndpoint, ObservationStart,
        RemoteObservationEvent,
    };
    use spatial_home::network::{HomeConnection, ScenePublisher, subscribe_scene};
    let pair = support::LocalPair::start().await?;
    let result = async {
        let mut built = built()?;
        set_points(&mut built.voxels, &[([0, 1, 0], 1.)]);
        let mut publisher = ScenePublisher::mount(&pair.a)?;
        publisher.publish(&built)?;
        let connection = HomeConnection::direct(&pair.a);
        let mut input = subscribe_scene(&pair.b, &connection).await?;
        let owner = pair.b.peer_id().to_string();
        let runtime = ComponentRuntime::new(owner.as_str());
        let endpoint = ComponentProtocolEndpoint::mount(pair.b.protocols(), runtime.clone())?;
        let clock = model::clock_entry(&owner, "occupancy-test");
        let mut config = Config::new(0.);
        config.interval = Duration::from_millis(100);
        config.stale_after = Duration::from_millis(200);
        let mut mapper = OccupancyMapper::mount(
            &runtime,
            &endpoint,
            input.product(),
            &owner,
            model::clock_ref(&clock),
            config,
        )?;
        tokio::time::timeout(Duration::from_secs(5), input.next()).await??;
        let mut poll = tokio::time::interval(Duration::from_millis(25));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                poll.tick().await;
                mapper.tick()?;
                if mapper.state().status == "ready" {
                    break;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        assert_eq!(
            value(mapper.state().layers.occupancy_2d.as_ref().unwrap(), 0, 0),
            OCCUPIED
        );
        let selected = select_cells(
            mapper.state(),
            ReadCells {
                cells: vec![[0, 0], [-2, 3], [0, 0]],
            },
        )?;
        assert!(!selected.complete_scene);
        assert_eq!(selected.requested_cells.len(), 2);
        let selected_grid = selected.scene.layers.occupancy_2d.unwrap();
        assert_eq!(selected_grid.cells.len(), 2);
        assert_eq!(value(&selected_grid, 0, 0), OCCUPIED);
        assert_eq!(value(&selected_grid, -40, 60), UNKNOWN);
        assert!(
            select_cells(
                mapper.state(),
                ReadCells {
                    cells: vec![[0, 0]; 65]
                }
            )
            .is_err()
        );
        assert!(!mapper.tick()?); // rate limited even with new input
        let encoded = serde_json::to_value(mapper.state())?;
        assert!(encoded.get("revision").is_none());
        assert!(encoded["layers"]["occupancy_2d"].get("revision").is_none());
        let client = ComponentProtocolClient::new(pair.a.protocols());
        let route = pair.b.listen_addresses()[0].clone();
        let auki_component_protocol::CatalogResponse::Snapshot { snapshot: catalog } = client
            .catalog_exact(pair.b.peer_id(), route.clone(), None)
            .await?
        else {
            panic!("no catalog")
        };
        let entry = catalog
            .components
            .iter()
            .find(|c| c.manifest.component_id == "voxel-to-occupancy-2d")
            .unwrap();
        assert_eq!(
            entry.current_product_inputs["source_scene"]
                .manifest
                .product
                .reference(),
            input.product().reference()
        );
        let response = client
            .invoke_exact::<ReadCells, CellSelection>(
                pair.b.peer_id(),
                route.clone(),
                entry.manifest.reference(),
                "read_cells",
                "occupancy-test",
                "read-1",
                Some(Duration::from_secs(2)),
                &ReadCells {
                    cells: vec![[0, 0], [5, 5]],
                },
            )
            .await?;
        assert!(!response.result.complete_scene);
        assert_eq!(
            response
                .result
                .scene
                .layers
                .occupancy_2d
                .unwrap()
                .cells
                .len(),
            2
        );
        let mut output = client
            .subscribe_product_exact::<OccupancyScene>(
                pair.b.peer_id(),
                route,
                mapper.product().reference(),
                ObservationStart::LatestExisting,
                BufferLimits::entries(2),
                |v| serde_json::to_vec(v).unwrap().len(),
            )
            .await?;
        let Some(RemoteObservationEvent::Observation(read)) =
            tokio::time::timeout(Duration::from_secs(5), output.next()).await??
        else {
            panic!("no occupancy output")
        };
        assert_eq!(read.payload.authority_peer_id, owner);
        assert_eq!(read.payload.source_product, input.product().reference());
        tokio::time::sleep(Duration::from_millis(250)).await;
        mapper.tick()?;
        assert_eq!(mapper.state().status, "stale");
        assert!(mapper.state().layers.occupancy_2d.is_some());
        // An empty source supplies no evidence to erase previously observed occupancy.
        set_points(&mut built.voxels, &[]);
        publisher.publish(&built)?;
        tokio::time::timeout(Duration::from_secs(5), input.next()).await??;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                poll.tick().await;
                mapper.tick()?;
                if mapper.state().status == "ready" {
                    break;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        assert_eq!(
            value(mapper.state().layers.occupancy_2d.as_ref().unwrap(), 0, 0),
            OCCUPIED
        );
        output.close().await?;
        drop(mapper);
        input.close().await?;
        endpoint.close().await?;
        publisher.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(pair.shutdown().await)
}
