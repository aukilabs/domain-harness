//! Read-only Galbot telemetry. All transforms are source -> destination; no
//! extrapolation, portal service calls, robot operations or map journal writes.
use crate::domain_portals::{DOMAIN, FRAME};
use anyhow::{Result, anyhow, bail, ensure};
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ObservationRequest, ObservationSelection,
    ObservationStart, RemoteObservationEvent,
};
use auki_components::{BufferLimits, OutputManifest, ProductReference};
use auki_odometry::{Pose, PoseUpdate, Tracking};
use auki_registry::{CoordinateConvention, FrameRegistryEntry};
use auki_scenegraph::MapSnapshot;
use auki_sdk::{AukiPeer, Multiaddr, PeerId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

const STALE: Duration = Duration::from_secs(3);
const ALIGNMENT_SCHEMA: &str = "galbot.map-alignment/v1";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerConfig {
    pub peer_id: String,
    #[serde(default)]
    pub routes: Vec<String>,
    #[serde(default)]
    pub lidar: Option<crate::runner_voxels::LidarConfig>,
}
impl RunnerConfig {
    pub fn validate(&self) -> Result<()> {
        self.peer_id.parse::<PeerId>()?;
        if let Some(lidar) = &self.lidar {
            lidar.validate()?;
        }
        ensure!(self.routes.len() <= 16, "too many runner routes");
        for route in &self.routes {
            route.parse::<Multiaddr>()?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MapAlignment {
    #[serde(flatten)]
    pub transform: PoseUpdate,
    pub domain_id: String,
    pub map_id: String,
}
#[derive(Clone, Debug)]
pub struct RunnerState {
    pub value: Value,
    received: Option<Instant>,
}
impl Default for RunnerState {
    fn default() -> Self {
        Self {
            value: json!({"status":"disabled","pose":null}),
            received: None,
        }
    }
}
impl RunnerState {
    pub fn view(&self) -> Value {
        let mut value = self.value.clone();
        if let Some(received) = self.received {
            value["age_ms"] = json!(received.elapsed().as_millis() as u64);
            if value["status"] == "live" && received.elapsed() > STALE {
                value["status"] = json!("stale");
            }
        }
        value
    }
}
fn zup_id() -> String {
    format!("domain.{DOMAIN}.zup")
}
fn zup_frame(peer: &str) -> FrameRegistryEntry {
    FrameRegistryEntry::in_convention(peer, zup_id(), CoordinateConvention::ZUpRightForward)
}
fn basis() -> Pose {
    let h = std::f64::consts::FRAC_1_SQRT_2;
    Pose {
        translation: [0.; 3],
        rotation_xyzw: [-h, 0., 0., h],
    }
}
/// a(b(point)); unit Hamilton quaternions in xyzw order.
pub fn compose(a: &Pose, b: &Pose) -> Pose {
    let [x, y, z, w] = a.rotation_xyzw;
    let [i, j, k, r] = b.rotation_xyzw;
    let v = b.translation;
    let t = [
        2. * (y * v[2] - z * v[1]),
        2. * (z * v[0] - x * v[2]),
        2. * (x * v[1] - y * v[0]),
    ];
    let cross = [
        y * t[2] - z * t[1],
        z * t[0] - x * t[2],
        x * t[1] - y * t[0],
    ];
    Pose {
        translation: std::array::from_fn(|n| a.translation[n] + v[n] + w * t[n] + cross[n]),
        rotation_xyzw: [
            w * i + x * r + y * k - z * j,
            w * j - x * k + y * r + z * i,
            w * k + x * j - y * i + z * r,
            w * r - x * i - y * j - z * k,
        ],
    }
}
fn validate_output(
    update: &PoseUpdate,
    output: &OutputManifest,
    peer: &str,
    schema: &str,
) -> Result<()> {
    update.validate()?;
    ensure!(
        output.peer_id == peer && output.payload.schema() == schema,
        "unexpected pose producer/schema"
    );
    ensure!(
        update.contract.clock == output.clock && output.clock.peer_id == peer,
        "pose capture clock mismatch"
    );
    ensure!(
        update.contract.from_frame.peer_id == peer && update.contract.to_frame.peer_id == peer,
        "pose frame owner mismatch"
    );
    Ok(())
}
pub fn world_pose(odom: &PoseUpdate, alignment: &MapAlignment, peer: &str) -> Result<Option<Pose>> {
    odom.validate()?;
    alignment.transform.validate()?;
    let o = &odom.contract;
    let a = &alignment.transform.contract;
    ensure!(
        alignment.domain_id == DOMAIN && !alignment.map_id.is_empty(),
        "alignment Domain mismatch"
    );
    ensure!(
        o.from_frame == FrameRegistryEntry::ros_body(peer, "robot.base"),
        "unexpected robot base frame"
    );
    ensure!(
        o.to_frame
            == FrameRegistryEntry::ros_body(peer, format!("robot.odometry.{}", o.session_id)),
        "unexpected odometry frame"
    );
    ensure!(
        a.to_frame == zup_frame(peer),
        "unexpected calibrated map frame"
    );
    ensure!(
        o.to_frame == a.from_frame && o.session_id == a.session_id && o.clock == a.clock,
        "odometry and alignment frame/session/clock mismatch"
    );
    match (&odom.tracking, &alignment.transform.tracking) {
        (Tracking::Valid { pose: o }, Tracking::Valid { pose: a }) => {
            Ok(Some(compose(&basis(), &compose(a, o))))
        }
        _ => Ok(None),
    }
}
/// Same Domain alone is insufficient. Compare all manually supplied anchors,
/// allowing only the precision lost by the user's rounded pose table.
pub fn verify_reference(
    reference: &MapSnapshot,
    home: &MapSnapshot,
    alignment: &MapAlignment,
) -> Result<()> {
    reference.validate()?;
    let map = &reference.scenegraph.map;
    ensure!(
        map.map_id == format!("{}-reference", alignment.map_id)
            && map.domain_reference.as_deref() == Some(DOMAIN)
            && map.frame.id == zup_id()
            && map.frame.convention == Some(CoordinateConvention::ZUpRightForward)
            && map.frame.meters_per_unit == 1.,
        "runner reference map contract mismatch"
    );
    ensure!(
        reference.scenegraph.anchors.len() == home.scenegraph.anchors.len(),
        "reference Portal count mismatch"
    );
    for (code, expected) in &home.scenegraph.anchors {
        let matches: Vec<_> = reference
            .scenegraph
            .anchors
            .values()
            .filter(|p| p.payload.rsplit('/').next() == Some(code.as_str()))
            .collect();
        ensure!(
            matches.len() == 1,
            "missing/ambiguous reference Portal {code}"
        );
        let p = matches[0];
        ensure!(
            (p.side_length_m - expected.side_length_m).abs() < 1e-6,
            "reference Portal size mismatch: {code}"
        );
        let [w, x, y, z] = p.pose_in_map.rotation_wxyz;
        let converted = compose(
            &basis(),
            &Pose {
                translation: p.pose_in_map.translation,
                rotation_xyzw: [x, y, z, w],
            },
        );
        ensure!(
            converted
                .translation
                .iter()
                .zip(expected.pose_in_map.translation)
                .all(|(a, b)| (a - b).abs() < 0.00011),
            "reference Portal position mismatch: {code}"
        );
        let [w, x, y, z] = expected.pose_in_map.rotation_wxyz;
        let q = [x, y, z, w];
        let distance = |sign: f64| {
            converted
                .rotation_xyzw
                .iter()
                .zip(q)
                .map(|(a, b)| (a - sign * b).abs())
                .fold(0., f64::max)
        };
        ensure!(
            distance(1.).min(distance(-1.)) < 0.000003,
            "reference Portal rotation mismatch: {code}"
        );
    }
    Ok(())
}

pub async fn monitor(
    peer: &AukiPeer,
    config: Option<&RunnerConfig>,
    rgbd: Option<&crate::network::HomeConnection>,
    home: &MapSnapshot,
    state: &watch::Sender<RunnerState>,
    mapping: &crate::runner_voxels::MappingTarget,
    stop: &CancellationToken,
) {
    let Some(config) = config else {
        stop.cancelled().await;
        return;
    };
    state.send_replace(RunnerState {
        value: json!({"status":"connecting","peer_id":config.peer_id,"pose":null}),
        received: None,
    });
    loop {
        let attempt = async {
            crate::home_pose_policy::unsupported("Home runner pose, alignment and mapping")?;
            let expected: PeerId = config.peer_id.parse()?;
            ensure!(
                peer.domain_id().to_string() == DOMAIN,
                "runner requires configured Home Domain"
            );
            let mut routes = config
                .routes
                .iter()
                .map(|s| s.parse())
                .collect::<Result<Vec<Multiaddr>, _>>()?;
            if routes.is_empty() {
                let candidates =
                    tokio::time::timeout(Duration::from_secs(15), peer.discover()).await??;
                for candidate in candidates {
                    if candidate.peer_id() == expected {
                        routes.extend_from_slice(candidate.routes());
                    }
                }
            }
            ensure!(
                !routes.is_empty(),
                "Galbot not discovered; waiting for its advertised route"
            );
            let mut error = anyhow!("no usable Galbot route");
            for route in routes {
                match follow(
                    peer,
                    expected,
                    route,
                    home,
                    state,
                    mapping,
                    config.lidar.as_ref(),
                    rgbd,
                    stop,
                )
                .await
                {
                    Ok(()) => return Ok(()),
                    Err(e) => error = e,
                }
                if stop.is_cancelled() {
                    return Ok(());
                }
            }
            Err(error)
        };
        let result = tokio::select! { _ = stop.cancelled() => break, result = attempt => result };
        if stop.is_cancelled() {
            break;
        }
        if let Err(error) = result {
            mapping
                .status
                .send_replace(json!({"status":"waiting","detail":error.to_string()}));
            // Never retain a pose after reference, session or transport validation fails.
            state.send_replace(RunnerState {
                value: json!({"status":"unavailable","peer_id":config.peer_id,
                "pose":null,"detail":error.to_string()}),
                received: None,
            });
        }
        tokio::select! { _ = stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
    }
    state.send_replace(RunnerState {
        value: json!({"status":"stopped","pose":null}),
        received: None,
    });
}

#[allow(clippy::too_many_arguments)]
async fn follow(
    peer: &AukiPeer,
    expected: PeerId,
    route: Multiaddr,
    home: &MapSnapshot,
    state: &watch::Sender<RunnerState>,
    mapping: &crate::runner_voxels::MappingTarget,
    lidar_config: Option<&crate::runner_voxels::LidarConfig>,
    rgbd: Option<&crate::network::HomeConnection>,
    stop: &CancellationToken,
) -> Result<()> {
    let client = ComponentProtocolClient::new(peer.protocols());
    let peer_id = expected.to_string();
    let setup = async {
        let CatalogResponse::Snapshot { snapshot } =
            client.catalog_exact(expected, route.clone(), None).await?
        else {
            bail!("runner catalog unavailable");
        };
        let product = |id: &str, by_component: bool| -> Result<ProductReference> {
            let entries: Vec<_> = snapshot
                .products
                .iter()
                .filter(|p| {
                    if by_component {
                        p.manifest.producer.component_id == id
                    } else {
                        p.manifest.product_id == id
                    }
                })
                .collect();
            ensure!(entries.len() == 1, "missing/ambiguous runner Product {id}");
            let p = entries[0];
            ensure!(
                p.manifest.peer_id == peer_id,
                "runner Product owner mismatch"
            );
            Ok(ProductReference {
                peer_id: peer_id.clone(),
                product_id: p.manifest.product_id.clone(),
                manifest_hash: p.manifest_hash.clone(),
            })
        };
        let alignment = product("odometry-map-alignment", false)?;
        let odometry = product("robot-odometry-history", false)?;
        let reference = client
            .observations_exact::<MapSnapshot>(
                expected,
                route.clone(),
                ObservationRequest {
                    product: product("legacy-reference-map", true)?,
                    selection: ObservationSelection::LatestExisting,
                },
            )
            .await?;
        let reference = reference
            .observations
            .last()
            .ok_or_else(|| anyhow!("runner reference map empty"))?
            .payload
            .clone();
        Ok::<_, anyhow::Error>((
            alignment,
            odometry,
            reference,
            if lidar_config.is_some() {
                Some(product("lidar.base-history", false)?)
            } else {
                None
            },
        ))
    };
    let (alignment_ref, odom_ref, reference, lidar_ref) =
        tokio::time::timeout(Duration::from_secs(25), setup).await??;
    let mut alignment = tokio::time::timeout(
        Duration::from_secs(15),
        client.subscribe_product_exact::<MapAlignment>(
            expected,
            route.clone(),
            alignment_ref,
            ObservationStart::LatestExisting,
            BufferLimits::entries(2),
            |_| 4096,
        ),
    )
    .await??;
    let work = async {
        let Some(RemoteObservationEvent::Observation(first)) = tokio::time::timeout(Duration::from_secs(10),alignment.next()).await??
            else { bail!("runner alignment unavailable"); };
        validate_output(&first.payload.transform,&alignment.product().producer,&peer_id,ALIGNMENT_SCHEMA)?;
        verify_reference(&reference,home,&first.payload)?;
        let mut pinned = first.payload;
        let mut alignment_time = first.timestamp_ns;
        let mut odom = tokio::time::timeout(Duration::from_secs(15),client.subscribe_product_exact::<PoseUpdate>(expected,route.clone(),odom_ref,
            ObservationStart::NewOnly,BufferLimits::entries(256),|_|4096)).await??;
        let work = async {
            let mut lidar_fetch=Box::pin(crate::runner_voxels::fetch(&client, expected, route.clone(), lidar_ref.clone()));
            let mut pending_lidar=None;
            let mut rgbd_fetch=Box::pin(crate::rgbd::fetch(&client,rgbd));
            let mut pending_rgbd=None;
            let mut mapping_failed=false;
            let mut previous = None;
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            let mut last = Instant::now();
            loop {
                tokio::select! {
                    _ = stop.cancelled() => return Ok(()),
                    _ = ticker.tick() => {
                        if last.elapsed() > STALE {
                            state.send_modify(|s| { s.value["status"]=json!("stale"); s.value["detail"]=json!("No recent odometry; last pose only"); });
                        }
                        ensure!(last.elapsed()<Duration::from_secs(15), "odometry stopped; reconnecting");
                    },
                    result = &mut lidar_fetch, if !mapping_failed => {
                        match result {
                            Ok(sample)=>{pending_lidar=Some(sample);set_lidar_status(&mapping.status,serde_json::json!({"status":"receiving","detail":"Matching LiDAR scan to capture-time robot pose"}));},
                            Err(error)=>{set_lidar_status(&mapping.status,serde_json::json!({"status":"waiting","detail":error.to_string()}));},
                        }
                        lidar_fetch=Box::pin(crate::runner_voxels::fetch(&client, expected, route.clone(), lidar_ref.clone()));
                    },
                    result = &mut rgbd_fetch, if !mapping_failed => {
                        match result {
                            Ok(sample)=>pending_rgbd=Some(sample),
                            Err(error)=>{mapping.status.send_modify(|s|s["rgbd"]=json!({"status":"waiting","detail":error.to_string()}));},
                        }
                        rgbd_fetch=Box::pin(crate::rgbd::fetch(&client,rgbd));
                    },
                    event = alignment.next() => {
                        let Some(RemoteObservationEvent::Observation(update)) = event? else {bail!("alignment ended or has a gap; rebinding");};
                        validate_output(&update.payload.transform,&alignment.product().producer,&peer_id,ALIGNMENT_SCHEMA)?;
                        ensure!(update.payload.transform.contract == pinned.transform.contract && update.timestamp_ns >= alignment_time,
                            "alignment contract/time changed; rebinding");
                        verify_reference(&reference,home,&update.payload)?;
                        pinned=update.payload;
                        alignment_time=update.timestamp_ns;
                        state.send_modify(|s| {s.value["status"]=json!("aligning");s.value["pose"]=Value::Null;});
                    },
                    event = odom.next() => {
                        let Some(RemoteObservationEvent::Observation(update)) = event? else {bail!("odometry ended or has a gap; rebinding");};
                        validate_output(&update.payload,&odom.product().producer,&peer_id,auki_odometry::POSE_SCHEMA)?;
                        ensure!(previous.is_none_or(|t| update.timestamp_ns>t), "nonmonotonic odometry timestamp");
                        previous=Some(update.timestamp_ns);
                        if update.timestamp_ns<alignment_time { continue; }
                        let pose=world_pose(&update.payload,&pinned,&peer_id)?;
                        last=Instant::now();
                        state.send_replace(RunnerState{value:json!({"status":if pose.is_some(){"live"}else{"lost"},
                            "peer_id":peer_id,"frame":FRAME,"pose":pose,"session_id":update.payload.contract.session_id,
                            "capture_timestamp_ns":update.timestamp_ns.to_string(),"sequence":update.sequence,
                            "clock":update.payload.contract.clock,"reference_portals":reference.scenegraph.anchors.len(),
                            "detail":"Calibrated map verified against Home’s manually supplied portals"}), received:Some(last)});
                    }
                }
                if let (Some(sample),Some(config))=(&pending_rgbd,rgbd) {
                    match crate::rgbd::place(sample,&config.peer_id,odom.product(),&pinned,alignment_time) {
                        Ok(Some(scan))=>{
                            let points=scan.hit_points_m.len();
                            let mut store=mapping.store.lock().map_err(|_|anyhow!("store unavailable"))?;
                            match store.integrate_prepared(scan) {
                                Ok(applied)=>{
                                    if applied {mapping.changes.send_replace(());}
                                    mapping.status.send_modify(|s|s["rgbd"]=json!({"status":"mapping","points":points,"colored_voxels":store.built.view.voxels.iter().filter(|v|v.linear_rgb.is_some()).count()}));
                                },
                                Err(error)=>{mapping_failed=true;mapping.status.send_modify(|s|s["rgbd"]=json!({"status":"blocked","detail":error.to_string()}));},
                            }
                            pending_rgbd=None;
                        },
                        Ok(None)=>{},
                        Err(error)=>{pending_rgbd=None;mapping.status.send_modify(|s|s["rgbd"]=json!({"status":"waiting","detail":error.to_string()}));},
                    }
                }
                if let (Some(sample),Some(config))=(&pending_lidar,lidar_config) {
                    match crate::runner_voxels::place(sample, odom.product(), &pinned, alignment_time, config) {
                        Ok(Some(scan))=>{
                            let field_names:Vec<_>=scan.observation.payload.measurement.fields.iter().map(|f|f.name.clone()).collect();
                            let result=mapping.store.lock().map_err(|_|anyhow!("store unavailable"))?.integrate_runner(scan);
                            match result {
                                Ok(applied)=>{
                                    let store=mapping.store.lock().map_err(|_|anyhow!("store unavailable"))?;
                                    if applied {mapping.changes.send_replace(());}
                                    set_lidar_status(&mapping.status,serde_json::json!({"status":"mapping","sensor":"lidar.base","integrated_scans":store.built.view.integrated_observations,"occupied_voxels":store.built.view.voxels.len(),"colored_voxels":store.built.view.voxels.iter().filter(|v|v.linear_rgb.is_some()).count(),"sensor_fields":field_names,"input_color_samples":store.mapping_inputs().last().map(|i|i.scan.hit_colors_srgb.iter().filter(|c|c.is_some()).count()),"detail":"Measured LiDAR · stationary scans · 0.3–5 m · 10 cm voxels"}));
                                },
                                Err(error)=>{mapping_failed=true;set_lidar_status(&mapping.status,serde_json::json!({"status":"blocked","detail":error.to_string()}));},
                            }
                            pending_lidar=None;
                        },
                        Ok(None)=>{}, // A newer scan replaces this pending sample after the next fetch.
                        Err(error)=>{pending_lidar=None;set_lidar_status(&mapping.status,serde_json::json!({"status":"waiting","detail":error.to_string()}));},
                    }
                }
            }
        }.await;
        let cleanup=odom.close().await;
        work.and(cleanup.map_err(Into::into))
    }.await;
    let cleanup = alignment.close().await;
    work.and(cleanup.map_err(Into::into))
}

fn set_lidar_status(status: &watch::Sender<Value>, mut value: Value) {
    if let Some(rgbd) = status.borrow().get("rgbd") {
        value["rgbd"] = rgbd.clone();
    }
    status.send_replace(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use auki_odometry::PoseContract;
    fn pair() -> (PoseUpdate, MapAlignment) {
        let peer = "robot";
        let clock = crate::model::clock_ref(&crate::model::clock_entry(peer, "session"));
        let identity = Pose {
            translation: [0.; 3],
            rotation_xyzw: [0., 0., 0., 1.],
        };
        let odom = PoseUpdate {
            contract: PoseContract {
                from_frame: FrameRegistryEntry::ros_body(peer, "robot.base"),
                to_frame: FrameRegistryEntry::ros_body(peer, "robot.odometry.session"),
                clock: clock.clone(),
                session_id: "session".into(),
            },
            tracking: Tracking::Valid {
                pose: identity.clone(),
            },
        };
        let alignment = MapAlignment {
            transform: PoseUpdate {
                contract: PoseContract {
                    from_frame: odom.contract.to_frame.clone(),
                    to_frame: zup_frame(peer),
                    clock,
                    session_id: "session".into(),
                },
                tracking: Tracking::Valid { pose: identity },
            },
            domain_id: DOMAIN.into(),
            map_id: "map".into(),
        };
        (odom, alignment)
    }
    #[test]
    fn composed_pose_rotates_translation_heading_and_up_axis() {
        let (mut o, mut a) = pair();
        o.tracking = Tracking::Valid {
            pose: Pose {
                translation: [1., 0., 0.],
                rotation_xyzw: [0., 0., 0., 1.],
            },
        };
        let h = std::f64::consts::FRAC_1_SQRT_2;
        a.transform.tracking = Tracking::Valid {
            pose: Pose {
                translation: [10., 20., 30.],
                rotation_xyzw: [0., 0., h, h],
            },
        };
        let world = world_pose(&o, &a, "robot").unwrap().unwrap();
        let close = |actual: [f64; 3], expected: [f64; 3]| {
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| (a - b).abs() < 1e-12)
            )
        };
        close(world.translation, [10., 30., -21.]);
        let heading = compose(
            &world,
            &Pose {
                translation: [1., 0., 0.],
                rotation_xyzw: [0., 0., 0., 1.],
            },
        );
        close(heading.translation, [10., 30., -22.]);
        let up = compose(
            &world,
            &Pose {
                translation: [0., 0., 1.],
                rotation_xyzw: [0., 0., 0., 1.],
            },
        );
        close(up.translation, [10., 31., -21.]);
    }
    #[test]
    fn mismatched_sessions_clocks_frames_and_lost_tracking_never_render() {
        let (o, a) = pair();
        for field in 0..5 {
            let mut bad = a.clone();
            match field {
                0 => bad.transform.contract.session_id = "reset".into(),
                1 => bad.transform.contract.clock.id = "another-clock".into(),
                2 => bad.transform.contract.from_frame.frame_id = "another-origin".into(),
                3 => bad.domain_id = "another-domain".into(),
                _ => bad.transform.contract.to_frame = zup_frame("wrong-owner"),
            }
            assert!(world_pose(&o, &bad, "robot").is_err());
        }
        let mut lost = o;
        lost.tracking = Tracking::Lost;
        assert!(world_pose(&lost, &a, "robot").unwrap().is_none());
    }
    #[test]
    fn reference_check_accepts_axis_conversion_and_rejects_changed_anchor() {
        let home = crate::domain_portals::load().unwrap();
        let (_, a) = pair();
        let mut scene = home.scenegraph.clone();
        scene.map.map_id = "map-reference".into();
        scene.map.frame = auki_scenegraph::MapFrame::in_convention(
            zup_id(),
            "reference",
            CoordinateConvention::ZUpRightForward,
            1.,
        )
        .unwrap();
        let mut inverse = basis();
        inverse.rotation_xyzw[0] *= -1.;
        for p in scene.anchors.values_mut() {
            let [w, x, y, z] = p.pose_in_map.rotation_wxyz;
            let pose = compose(
                &inverse,
                &Pose {
                    translation: p.pose_in_map.translation,
                    rotation_xyzw: [x, y, z, w],
                },
            );
            p.pose_in_map.translation = pose.translation;
            let [x, y, z, w] = pose.rotation_xyzw;
            p.pose_in_map.rotation_wxyz = [w, x, y, z];
            p.pose_in_map.to_frame_id = zup_id();
            p.payload = format!("HTTPS://R8.HR/{}", p.anchor_id);
        }
        verify_reference(&MapSnapshot::new(scene.clone()).unwrap(), &home, &a).unwrap();
        scene
            .anchors
            .values_mut()
            .next()
            .unwrap()
            .pose_in_map
            .translation[0] += 0.001;
        assert!(verify_reference(&MapSnapshot::new(scene).unwrap(), &home, &a).is_err());
    }
    #[test]
    fn freshness_uses_local_elapsed_time_and_keeps_last_pose_explicitly_stale() {
        let state = RunnerState {
            value: json!({"status":"live","pose":{"translation":[1,2,3]}}),
            received: Some(Instant::now() - Duration::from_secs(4)),
        };
        let view = state.view();
        assert_eq!(view["status"], "stale");
        assert_eq!(view["pose"], state.value["pose"]);
    }
}
