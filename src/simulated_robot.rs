//! Opt-in, isolated robot/Home peers. No DDS login, discovery, real robot or map writes.
use crate::{
    network::{HomeConnection, subscribe_scene_product},
    occupancy_fixture::FakeVoxelSource,
    occupancy_mapper::{Config, OccupancyMapper, OccupancyScene},
};
use anyhow::{Result, bail};
use auki_component_protocol::{ComponentProtocolEndpoint, RemoteObservationEvent};
use auki_components::ComponentRuntime;
use auki_p2p::{
    P2P_TOKEN_AUDIENCE, P2P_TOKEN_ISSUER, P2P_TOKEN_SCOPE, P2P_TOKEN_TYPE, P2PAccessClaims,
};
use auki_sdk::{
    AukiPeer, AukiPeerConfig, DdsVerificationKeys, ExternalAuthorityUpdate, Identity,
    SignedP2pCredential,
};
use chrono::{TimeZone, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct SimulationState {
    /// SDK peer inventory and catalogs fetched over authenticated P2P, not UI fixtures.
    pub network: Value,
    pub voxels: Option<Value>,
    pub occupancy: Option<Value>,
    pub source_snapshot: Option<Value>,
    pub occupancy_snapshot: Option<OccupancyScene>,
    pub status: String,
    pub detail: String,
}

async fn peer(domain: uuid::Uuid, signing: &EncodingKey, public: &[u8]) -> Result<AukiPeer> {
    let identity = Identity::generate();
    let now = Utc::now().timestamp() as u64;
    let expires = now + 30 * 60;
    let claims = P2PAccessClaims {
        token_type: P2P_TOKEN_TYPE.into(),
        iss: P2P_TOKEN_ISSUER.into(),
        aud: vec![P2P_TOKEN_AUDIENCE.into()],
        sub: uuid::Uuid::new_v4().to_string(),
        organization_id: None,
        peer_type: Some("simulation".into()),
        peer_id: identity.peer_id().to_string(),
        domain_ids: vec![domain.to_string()],
        scopes: vec![P2P_TOKEN_SCOPE.into()],
        application: None,
        iat: now,
        nbf: None,
        exp: expires,
    };
    let authority = ExternalAuthorityUpdate::new(
        domain,
        identity.peer_id(),
        DdsVerificationKeys::new(0, public.to_vec(), None),
        SignedP2pCredential::new(encode(&Header::new(Algorithm::ES256), &claims, signing)?)?,
        Utc.timestamp_opt(expires as i64, 0).unwrap(),
    );
    let config = AukiPeerConfig::new("http://127.0.0.1:9")?
        .direct_only()
        .with_listen_addresses(["/ip4/127.0.0.1/tcp/0".parse()?])?;
    let (peer, _) = AukiPeer::start_external(identity, authority, config).await?;
    Ok(peer)
}

/// Two robot peers and one Home peer communicate through loopback sockets.
pub async fn run(state: watch::Sender<SimulationState>, stop: CancellationToken) -> Result<()> {
    run_controlled(state, stop, CancellationToken::new()).await
}
pub async fn run_controlled(
    state: watch::Sender<SimulationState>,
    stop: CancellationToken,
    disconnect: CancellationToken,
) -> Result<()> {
    run_counted(state, stop, disconnect, 2).await
}
pub async fn run_counted(
    state: watch::Sender<SimulationState>,
    stop: CancellationToken,
    disconnect: CancellationToken,
    count: usize,
) -> Result<()> {
    anyhow::ensure!((1..=2).contains(&count), "robot count must be 1 or 2");
    state.send_modify(|s| s.status = "starting".into());
    let result = run_inner(&state, &stop, &disconnect, count).await;
    state.send_modify(|s| {
        s.status = if result.is_err() { "failed" } else { "stopped" }.into();
        s.detail = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        s.network["status"] = json!(s.status);
        if let Some(peers) = s.network["peers"].as_array_mut() {
            for peer in peers {
                peer["status"] = json!("disconnected");
            }
        }
    });
    result
}
async fn run_inner(
    state: &watch::Sender<SimulationState>,
    stop: &CancellationToken,
    disconnect: &CancellationToken,
    count: usize,
) -> Result<()> {
    let issuer = p256::SecretKey::random(&mut rand::rngs::OsRng);
    let signing = EncodingKey::from_ec_pem(issuer.to_pkcs8_pem(LineEnding::LF)?.as_bytes())?;
    let public = issuer.public_key().to_public_key_pem(LineEnding::LF)?;
    let domain = uuid::Uuid::new_v4();
    let robot = peer(domain, &signing, public.as_bytes()).await?;
    let home = match peer(domain, &signing, public.as_bytes()).await {
        Ok(home) => home,
        Err(e) => {
            robot.shutdown().await?;
            return Err(e);
        }
    };
    let robot2 = if count == 2 {
        match peer(domain, &signing, public.as_bytes()).await {
            Ok(peer) => Some(peer),
            Err(e) => {
                let _ = tokio::join!(robot.shutdown(), home.shutdown());
                return Err(e);
            }
        }
    } else {
        None
    };
    let result = connected(robot, robot2, &home, state, stop, disconnect).await;
    result.and(home.shutdown().await.map_err(Into::into))
}
async fn connected(
    robot: AukiPeer,
    robot2: Option<AukiPeer>,
    home: &AukiPeer,
    state: &watch::Sender<SimulationState>,
    stop: &CancellationToken,
    disconnect: &CancellationToken,
) -> Result<()> {
    let count = if robot2.is_some() { 2 } else { 1 };
    let mut robots = [Some(robot), robot2];
    let robot = robots[0].as_ref().unwrap();
    let robot2 = robots[1].as_ref();
    let lifecycles = [Some(robot.lifecycle()), robot2.map(AukiPeer::lifecycle)];
    let connections = [
        Some(HomeConnection::direct(robot)),
        robot2.map(HomeConnection::direct),
    ];
    let owner = robot.peer_id().to_string();
    let runtime = ComponentRuntime::new(owner.as_str());
    let endpoint = ComponentProtocolEndpoint::mount(robot.protocols(), runtime.clone())?;
    let owner2 = robot2.map(|r| r.peer_id().to_string()).unwrap_or_default();
    let runtime2 = ComponentRuntime::new(owner2.as_str());
    let endpoint2 = if let Some(robot2) = robot2 {
        match ComponentProtocolEndpoint::mount(robot2.protocols(), runtime2.clone()) {
            Ok(endpoint) => Some(endpoint),
            Err(e) => {
                endpoint.close().await?;
                return Err(e.into());
            }
        }
    } else {
        None
    };
    let clock = auki_time::SessionClock::new(&owner, uuid::Uuid::new_v4().to_string(), "session");
    let (mut time, registry) = clock_product(robot, &runtime, &endpoint, &clock)?;
    let clock2 = auki_time::SessionClock::new(&owner2, uuid::Uuid::new_v4().to_string(), "session");
    let mut time2 = if let (Some(robot2), Some(endpoint2)) = (robot2, &endpoint2) {
        Some(clock_product(robot2, &runtime2, endpoint2, &clock2)?)
    } else { None };
    let result = async {
        let mut source = FakeVoxelSource::new(&runtime, &owner, clock.clone())?;
        endpoint.export_product(&source.product())?;
        source.publish_patrol(0.)?;
        let mut source2 = if let Some(endpoint2) = &endpoint2 {
        let mut source2 = FakeVoxelSource::new(&runtime2, &owner2, clock2.clone())?;
        endpoint2.export_product(&source2.product())?;
        source2.publish_patrol_phase(0., std::f64::consts::PI)?;
        Some(source2) } else { None };
        let mut input = subscribe_scene_product(home, connections[0].as_ref().unwrap(), "fake-voxels-scene").await?;
        let work = async {
            let mut input2 = if let Some(connection) = &connections[1] { Some(subscribe_scene_product(home, connection, "fake-voxels-scene").await?) } else {None};
            let work = async {
            let home_owner = home.peer_id().to_string();
            let home_runtime = ComponentRuntime::new(home_owner.as_str());
            let home_endpoint = ComponentProtocolEndpoint::mount(home.protocols(), home_runtime.clone())?;
            let home_clock = auki_time::SessionClock::new(&home_owner, uuid::Uuid::new_v4().to_string(), "session");
            let (mut home_time, home_registry) = clock_product(home, &home_runtime, &home_endpoint, &home_clock)?;
            let work = async {
                let mut combined = FakeVoxelSource::new(&home_runtime, &home_owner, home_clock.clone())?;
                home_endpoint.export_product(&combined.product())?;
                let mut mapper = OccupancyMapper::mount(&home_runtime, &home_endpoint, &combined.product(), &home_owner, home_clock.clone(), Config::new(0.))?;
                let client = auki_component_protocol::ComponentProtocolClient::new(home.protocols());
                let mut devices = Vec::new();
                // Enumerate the SDK's actual known peers, using the same inventory as live Home.
                let inventory = home.known_peers().snapshot();
                for known in inventory.peers() {
                    let id = known.peer_id();
                    let (target, name) = if id.to_string() == owner { (&connections[0],"Robot 1") }
                        else if id.to_string() == owner2 { (&connections[1],"Robot 2") } else { continue; };
                    let auki_component_protocol::CatalogResponse::Snapshot { snapshot } = client.catalog_exact(id,target.as_ref().unwrap().routes[0].parse()?,None).await?
                        else { bail!("robot catalog unavailable"); };
                    devices.push(json!({"peer_id":id.to_string(),"name":name,"status":"connected","catalog":snapshot,"received_observations":0}));
                }
                anyhow::ensure!(devices.len()==count,"SDK inventory does not contain the requested robot peers");
                state.send_modify(|s| s.network = json!({"enabled":true,"simulation":true,"status":"Simulation · running","connection":HomeConnection::direct(home),"catalog":home_runtime.catalog().snapshot(),"peers":devices,"detail":"Authenticated loopback peers; synthetic observation payloads"}));
                let mut latest: [Option<std::sync::Arc<crate::network::CanonicalScene>>; 2] = [None, None];
                let start = Instant::now();
                let mut publish = tokio::time::interval(Duration::from_secs(1));
                publish.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut tick = tokio::time::interval(Duration::from_millis(50));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                println!("Synthetic robot peers {owner}, {owner2} → Home peer {home_owner} (loopback only)");
                let mut disconnected = false;
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,
                        _ = async { tokio::select! { _ = disconnect.cancelled() => {}, _ = lifecycles[0].as_ref().unwrap().wait_stopped() => {}, _ = async { if let Some(lifecycle) = &lifecycles[1] { lifecycle.wait_stopped().await; } else { std::future::pending::<()>().await; } } => {} } }, if !disconnected => {
                            for robot in &mut robots { if let Some(robot) = robot.take() { robot.shutdown().await?; } }
                            disconnected = true;
                            state.send_modify(|s| {
                                s.status = "disconnected".into();
                                s.detail = "Robot publishers disconnected; Home retains accumulated occupancy".into();
                                s.voxels = None; s.source_snapshot = None;
                                s.network["status"] = json!("Home running · robots disconnected");
                                if let Some(peers) = s.network["peers"].as_array_mut() { for peer in peers { peer["status"] = json!("disconnected"); } }
                                if let Some(view) = &mut s.occupancy { view["local_mapping"] = json!({}); }
                            });
                        },
                        exit = home.wait_stopped() => bail!("simulation Home peer stopped: {exit:?}"),
                        _ = publish.tick(), if !disconnected => {
                            let seconds = start.elapsed().as_secs() as f64;
                            source.publish_patrol(seconds)?;
                            if let Some(source2) = &mut source2 {source2.publish_patrol_phase(seconds, std::f64::consts::PI)?;}
                        },
                        _ = tick.tick() => {
                            home_time.sample(&crate::capture_time::SessionUtcClock(home_clock.clone()))?;
                            if !disconnected {
                                time.sample(&crate::capture_time::SessionUtcClock(clock.clone()))?;
                                if let Some((time2, _)) = &mut time2 { time2.sample(&crate::capture_time::SessionUtcClock(clock2.clone()))?; }
                            }
                            if mapper.tick()? {
                                let snapshot = mapper.state().clone();
                                state.send_modify(|s| {
                                    if let Some(v) = &s.voxels { s.occupancy = Some(occupancy_view(v, &snapshot)); }
                                    else if let Some(v) = &mut s.occupancy { v["occupancy_status"] = json!(snapshot.status); }
                                    s.occupancy_snapshot = Some(snapshot);
                                });
                            }
                        },
                        (index, event) = async { tokio::select! { e = input.next() => (0,e), e = async { if let Some(input2) = &mut input2 {input2.next().await} else {std::future::pending().await} } => (1,e) } }, if !disconnected => match event {
                            Ok(Some(RemoteObservationEvent::Observation(observation))) => {
                                observation.payload.validate(if index == 0 { &owner } else { &owner2 })?;
                                state.send_modify(|s| {
                                    if let Some(devices) = s.network["peers"].as_array_mut() {
                                        for device in devices {
                                            if device["peer_id"] == observation.payload.authority_peer_id {
                                                device["received_observations"] = json!(device["received_observations"].as_u64().unwrap_or(0)+1);
                                            }
                                        }
                                    }
                                });
                                latest[index] = Some(observation.payload);
                                let Some(a) = &latest[0] else {continue;};
                                let mut sources = vec![a.clone()];
                                if count == 2 {
                                    let Some(b) = &latest[1] else {continue;};
                                    if a.registries["local_mapping"]["robot_pose"]["simulation_seconds"] != b.registries["local_mapping"]["robot_pose"]["simulation_seconds"] { continue; }
                                    sources.push(b.clone());
                                }
                                let scene = combined.publish_combined(&sources)?;
                                let mut view = crate::scene_mirror::render_scene(&scene)?;
                                // Fixture portal calibrations only provide valid SDK contracts, not room geometry.
                                view["portals"] = json!([]);
                                view["simulation"] = json!(true);
                                view["integrated_observations"] = json!(start.elapsed().as_secs() + 1);
                                view["remote_status"] = json!({"status":"connected","age_ms":0});
                                state.send_modify(|s| {
                                    s.source_snapshot = serde_json::to_value(&*scene).ok();
                                    if let Some(snapshot) = &s.occupancy_snapshot {
                                        s.occupancy = Some(occupancy_view(&view, snapshot));
                                    }
                                    s.voxels = Some(view);
                                    s.status = "running".into();
                                    s.detail = format!("{count} robot peer(s) → Home map · 1 Hz · no real robot commands");
                                });
                            },
                            Ok(Some(RemoteObservationEvent::Gap(_))) => {},
                            _ => { disconnect.cancel(); },
                        }
                    }
                }
                Ok::<_, anyhow::Error>(())
            }.await;
            let close_registry = home_registry.close().await;
            let close_endpoint = home_endpoint.close().await;
            work.and(close_registry.map_err(Into::into)).and(close_endpoint.map_err(Into::into))
            }.await;
            work.and(if let Some(mut input2) = input2 {input2.close().await.map_err(Into::into)} else {Ok(())})
        }.await;
        work.and(input.close().await.map_err(Into::into))
    }.await;
    let registry_result = registry.close().await;
    let registry2_result = if let Some((_, registry)) = time2 { registry.close().await } else { Ok(()) };
    let a = endpoint.close().await;
    let b = if let Some(endpoint2) = endpoint2 {
        endpoint2.close().await
    } else {
        Ok(())
    };
    for robot in robots.into_iter().flatten() {
        robot.shutdown().await?;
    }
    result.and(registry_result.map_err(Into::into)).and(registry2_result.map_err(Into::into)).and(a.map_err(Into::into)).and(b.map_err(Into::into))
}

fn clock_product(peer: &AukiPeer, runtime: &ComponentRuntime, endpoint: &ComponentProtocolEndpoint,
    clock: &auki_time::SessionClock) -> Result<(crate::capture_time::Publisher, auki_protocols::registry::RegistryEndpoint)> {
    let mut publisher = crate::capture_time::Publisher::mount(runtime, endpoint, clock.registry_entry())?;
    publisher.sample(&crate::capture_time::SessionUtcClock(clock.clone()))?;
    let definitions = crate::product_registry::Definitions::default();
    crate::product_registry::clock(&definitions, &clock.registry_entry())?;
    crate::product_registry::clock(&definitions, &publisher.contract.to_clock)?;
    Ok((publisher, crate::product_registry::mount(peer, runtime.clone(), definitions)?))
}

/// Display adapter only. The exported Product retains its typed grid/cell schema.
fn occupancy_view(source: &Value, occupancy: &OccupancyScene) -> Value {
    let mut view = source.clone();
    view["voxels"] = json!([]);
    view["occupancy_tiles"] = json!([]);
    view["authority_peer_id"] = json!(occupancy.authority_peer_id);
    view["occupancy_status"] = json!(occupancy.status);
    if let Some(grid) = &occupancy.layers.occupancy_2d {
        let tiles: Vec<_> = grid
            .cells
            .iter()
            .flat_map(|cell| {
                cell.values.iter().enumerate().map(move |(i, value)| {
                    let mut center = [0.; 3];
                    center[grid.horizontal_axes[0]] = grid.origin_uv_m[0]
                        + (cell.index[0] as f64 * 20. + (i % 20) as f64 + 0.5) * grid.resolution_m;
                    center[grid.horizontal_axes[1]] = grid.origin_uv_m[1]
                        + (cell.index[1] as f64 * 20. + (i / 20) as f64 + 0.5) * grid.resolution_m;
                    center[grid.up_axis] = grid.floor_m;
                    json!({"center":center,"value":value})
                })
            })
            .collect();
        view["occupancy_tiles"] = json!(tiles);
    }
    view
}
