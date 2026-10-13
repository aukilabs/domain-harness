//! Robot-side Operable: point an arm at a point given in a known frame.
//!
//! `point_at` validates the request, resolves the point into `robot.base`
//! from the runner's live control snapshot and accepts or rejects it. One
//! request at a time: while an accepted request points and holds, others are
//! rejected. Milestone 1 does not move the arm (`executes: false`); the busy
//! window is still the request's nominal duration so callers see real
//! contention behaviour before motion is attached.
use crate::local_mapper::{compose, from_rigid, identity, inverse};
use anyhow::{Result, anyhow, bail, ensure};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;
use auki_odometry::Pose;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const COMPONENT: &str = "arm-pointer";
pub const OPERATION: &str = "point_at";
pub const INSTRUCTION: &str = "spatial.point-at/v1";
pub const RESULT: &str = "spatial.point-at-accepted/v1";
/// The arm holds the pointing pose this long before returning.
pub const HOLD: Duration = Duration::from_secs(3);
/// Nominal move-out plus return time, until real motion reports its own.
const MOTION_ALLOWANCE: Duration = Duration::from_secs(8);
/// The frame name for points already expressed in the robot body frame.
pub const BASE_FRAME: &str = "robot.base";
const MIN_DISTANCE_M: f64 = 0.3;
const MAX_DISTANCE_M: f64 = 30.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmChoice {
    /// The arm on the target's side of the robot.
    #[default]
    Auto,
    Left,
    Right,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointAt {
    pub request_id: String,
    /// `robot.base`, the robot's map frame, or the Home scene frame the robot
    /// is currently aligned to (e.g. `local-scene:bb-lab:portal-280UO1U1RPC`).
    pub frame: String,
    pub point_m: [f64; 3],
    #[serde(default)]
    pub arm: ArmChoice,
}
impl ContractType for PointAt {
    const DATATYPE: &'static str = INSTRUCTION;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PointAtAccepted {
    pub request_id: String,
    pub arm: ArmChoice,
    pub frame: String,
    /// The target in `robot.base` at acceptance: X right, Y forward, Z up (m).
    pub point_base_m: [f64; 3],
    /// The target in the robot's map frame, where it stays fixed while held.
    pub point_map_m: [f64; 3],
    pub map_frame: String,
    pub distance_m: f64,
    /// Horizontal angle from straight ahead; positive is to the robot's left.
    pub bearing_deg: f64,
    /// Angle above the horizontal plane through the base origin.
    pub elevation_deg: f64,
    pub hold_s: f64,
    /// False until arm motion is attached; the request is resolved only.
    pub executes: bool,
}
impl ContractType for PointAtAccepted {
    const DATATYPE: &'static str = RESULT;
}

pub fn contract() -> OperableContract {
    OperableContract {
        name: OPERATION.into(),
        instruction: INSTRUCTION.into(),
        result: RESULT.into(),
        exposure: Exposure::Cluster,
    }
}

fn point_through(pose: &Pose, point: [f64; 3]) -> Result<[f64; 3]> {
    let mut at = identity();
    at.translation = point;
    Ok(compose(pose, &at)?.translation)
}

/// Resolve `request` against a `spatial.robot-control-state/v1` snapshot.
pub fn resolve(request: &PointAt, snapshot: &Value) -> Result<PointAtAccepted> {
    uuid::Uuid::parse_str(&request.request_id).map_err(|_| anyhow!("request_id must be a UUID"))?;
    ensure!(request.point_m.iter().all(|v| v.is_finite()), "point must be finite");
    let mapping = &snapshot["registries"]["local_mapping"];
    let robot = &mapping["robot_pose"];
    ensure!(robot.is_object(), "robot pose unavailable (tracking lost or mapper not ready)");
    let map_frame = robot["frame"]
        .as_str()
        .ok_or_else(|| anyhow!("robot pose has no map frame"))?
        .to_owned();
    // robot_pose maps robot.base into the map frame.
    let base_to_map: Pose = serde_json::from_value(robot["pose"].clone())?;
    let map_to_base = inverse(&base_to_map)?;
    let (point_map, point_base) = if request.frame == BASE_FRAME {
        (point_through(&base_to_map, request.point_m)?, request.point_m)
    } else if request.frame == map_frame {
        (request.point_m, point_through(&map_to_base, request.point_m)?)
    } else {
        let transform = &snapshot["registries"]["frame_transform"];
        let scene = transform["target_frame"]["id"].as_str();
        if scene != Some(request.frame.as_str()) {
            let known = [Some(BASE_FRAME), Some(map_frame.as_str()), scene];
            bail!(
                "no transform from {} to this robot; known frames: {}",
                request.frame,
                known.into_iter().flatten().collect::<Vec<_>>().join(", ")
            );
        }
        ensure!(
            transform["validity"] == "valid",
            "alignment to {} is {}: {}",
            request.frame,
            transform["validity"].as_str().unwrap_or("unavailable"),
            transform["reason"].as_str().unwrap_or("")
        );
        let map_to_scene = from_rigid(&serde_json::from_value(transform["transform"].clone())?);
        let point_map = point_through(&inverse(&map_to_scene)?, request.point_m)?;
        (point_map, point_through(&map_to_base, point_map)?)
    };
    let [x, y, z] = point_base;
    let distance = (x * x + y * y + z * z).sqrt();
    ensure!(
        (MIN_DISTANCE_M..=MAX_DISTANCE_M).contains(&distance),
        "target is {distance:.2} m from the robot; must be {MIN_DISTANCE_M}..{MAX_DISTANCE_M} m"
    );
    ensure!(y > 0., "target is behind the robot (forward {y:.2} m); turn the robot first");
    let arm = match request.arm {
        ArmChoice::Auto if x > 0. => ArmChoice::Right,
        ArmChoice::Auto => ArmChoice::Left,
        chosen => chosen,
    };
    Ok(PointAtAccepted {
        request_id: request.request_id.clone(),
        arm,
        frame: request.frame.clone(),
        point_base_m: point_base,
        point_map_m: point_map,
        map_frame,
        distance_m: distance,
        bearing_deg: (-x).atan2(y).to_degrees(),
        elevation_deg: z.atan2((x * x + y * y).sqrt()).to_degrees(),
        hold_s: HOLD.as_secs_f64(),
        executes: false,
    })
}

/// One request at a time; others are rejected until it finishes.
#[derive(Default)]
pub struct Busy {
    active: Option<(String, Instant)>,
}
impl Busy {
    pub fn claim(&mut self, request_id: &str, now: Instant) -> Result<()> {
        if let Some((id, until)) = &self.active
            && now < *until
        {
            bail!(
                "busy pointing for request {id} ({:.1} s left)",
                (*until - now).as_secs_f64()
            );
        }
        self.active = Some((request_id.into(), now + MOTION_ALLOWANCE + HOLD));
        Ok(())
    }
}

pub struct ArmPointer {
    _component: Component,
    _operation: Operable<PointAt, PointAtAccepted>,
}
impl ArmPointer {
    /// `snapshot` returns the runner's live `spatial.robot-control-state/v1`.
    pub fn mount(
        runtime: &ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        snapshot: impl Fn() -> Option<Value> + Send + Sync + 'static,
    ) -> Result<Self> {
        let component = runtime.component(ComponentSpec::new(COMPONENT).operable(contract()))?;
        let busy = Arc::new(Mutex::new(Busy::default()));
        let operation = component.operable_ordered(
            OPERATION,
            InvocationOrdering::SerialInAcceptanceOrder,
            |ctx| !ctx.caller_peer_id.is_empty(),
            move |ctx, request: PointAt| {
                let rejected = |e: anyhow::Error| InvocationError::Rejected(format!("{e:#}"));
                let state = snapshot().ok_or_else(|| rejected(anyhow!("robot control state not ready")))?;
                let accepted = resolve(&request, &state).map_err(rejected)?;
                busy.lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?
                    .claim(&request.request_id, Instant::now())
                    .map_err(rejected)?;
                eprintln!(
                    "point_at {} from {}: {:?} arm, {:.2} m, bearing {:.0}°, elevation {:.0}° (not executed)",
                    accepted.request_id, ctx.caller_peer_id, accepted.arm, accepted.distance_m,
                    accepted.bearing_deg, accepted.elevation_deg
                );
                Ok(accepted)
            },
        )?;
        component.expose()?;
        endpoint.export_operable(&operation)?;
        Ok(Self { _component: component, _operation: operation })
    }
}

/// Home side: invoke `point_at` on a robot peer on behalf of the viewer.
pub struct Request {
    pub peer_id: String,
    pub instruction: PointAt,
    pub reply: tokio::sync::oneshot::Sender<Result<PointAtAccepted, String>>,
}

pub async fn serve(
    peer: &auki_sdk::AukiPeer,
    catalogs: tokio::sync::watch::Receiver<crate::device_catalogs::Catalogs>,
    mut requests: tokio::sync::mpsc::Receiver<Request>,
    stop: &tokio_util::sync::CancellationToken,
) {
    let client = auki_component_protocol::ComponentProtocolClient::new(peer.protocols());
    loop {
        let request = tokio::select! {
            _ = stop.cancelled() => break,
            r = requests.recv() => match r { Some(r) => r, None => break },
        };
        let row = catalogs.borrow().get(&request.peer_id).cloned();
        let result = invoke(&client, row, &request).await.map_err(|e| format!("{e:#}"));
        let _ = request.reply.send(result);
    }
}

async fn invoke(
    client: &auki_component_protocol::ComponentProtocolClient,
    row: Option<Value>,
    request: &Request,
) -> Result<PointAtAccepted> {
    let row = row.ok_or_else(|| anyhow!("robot {} is not in Home's peer catalogs", request.peer_id))?;
    let snapshot: CatalogSnapshot = serde_json::from_value(row["catalog"].clone())
        .map_err(|_| anyhow!("robot catalog unavailable"))?;
    let entry = snapshot
        .components
        .iter()
        .find(|c| c.manifest.component_id == COMPONENT && c.manifest.operables.iter().any(|o| *o == contract()))
        .ok_or_else(|| anyhow!("robot does not offer {COMPONENT}/{OPERATION}"))?;
    let peer: auki_sdk::PeerId = request.peer_id.parse()?;
    let (reference, id, deadline) = (entry.manifest.reference(), request.instruction.request_id.clone(), Some(Duration::from_secs(5)));
    let reply = match row["catalog_route"].as_str() {
        Some(route) => client
            .invoke_exact::<_, PointAtAccepted>(peer, route.parse()?, reference, OPERATION, "home-viewer", id, deadline, &request.instruction)
            .await?,
        None => client
            .invoke::<_, PointAtAccepted>(peer, reference, OPERATION, "home-viewer", id, deadline, &request.instruction)
            .await?,
    };
    Ok(reply.result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_mapper::rigid;
    use serde_json::json;

    const MAP: &str = "bracketbot-local:3f1d7c2e-0000-4000-8000-000000000000:zup";
    const SCENE: &str = "local-scene:bb-lab:portal-280UO1U1RPC";

    fn yaw(deg: f64, t: [f64; 3]) -> Pose {
        let h = deg.to_radians() / 2.;
        Pose { translation: t, rotation_xyzw: [0., 0., h.sin(), h.cos()] }
    }
    /// Robot 2 m forward in its map, turned 90° left; scene = map shifted 10 m in x.
    fn snapshot(validity: &str) -> Value {
        let scene_from_map = Pose { translation: [10., 0., 0.], rotation_xyzw: [0., 0., 0., 1.] };
        json!({"registries":{
            "local_mapping":{"robot_pose":{"frame":MAP,"capture_timestamp_ns":1,"pose":yaw(90., [0., 2., 0.])}},
            "frame_transform":{"validity":validity,"reason":"test","target_frame":{"id":SCENE},
                "transform":rigid(&scene_from_map, MAP, SCENE)}}})
    }
    fn request(frame: &str, point: [f64; 3]) -> PointAt {
        PointAt { request_id: uuid::Uuid::new_v4().to_string(), frame: frame.into(), point_m: point, arm: ArmChoice::Auto }
    }
    fn close(a: [f64; 3], b: [f64; 3]) {
        assert!(a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9), "{a:?} vs {b:?}");
    }

    #[test]
    fn map_and_scene_points_resolve_into_robot_base() {
        // The robot faces map -x (base +Y). A point 1 m further along map -x
        // and 0.5 m up is straight ahead of it.
        let map = resolve(&request(MAP, [-1., 2., 0.5]), &snapshot("valid")).unwrap();
        close(map.point_base_m, [0., 1., 0.5]);
        assert!(map.bearing_deg.abs() < 1e-9 && (map.elevation_deg - 26.565).abs() < 1e-3);
        // The same physical point named in the scene frame.
        let scene = resolve(&request(SCENE, [9., 2., 0.5]), &snapshot("valid")).unwrap();
        close(scene.point_base_m, [0., 1., 0.5]);
        close(scene.point_map_m, [-1., 2., 0.5]);
        // robot.base passes through and reports its map position.
        let base = resolve(&request(BASE_FRAME, [0., 1., 0.5]), &snapshot("valid")).unwrap();
        close(base.point_map_m, [-1., 2., 0.5]);
    }

    #[test]
    fn arm_side_bearing_and_reach_rules() {
        let left = resolve(&request(BASE_FRAME, [-0.5, 1., 1.]), &snapshot("valid")).unwrap();
        assert_eq!(left.arm, ArmChoice::Left);
        assert!(left.bearing_deg > 0.);
        let right = resolve(&request(BASE_FRAME, [0.5, 1., 1.]), &snapshot("valid")).unwrap();
        assert_eq!(right.arm, ArmChoice::Right);
        let mut forced = request(BASE_FRAME, [0.5, 1., 1.]);
        forced.arm = ArmChoice::Left;
        assert_eq!(resolve(&forced, &snapshot("valid")).unwrap().arm, ArmChoice::Left);
        for bad in [[0., -1., 1.], [0., 0.1, 0.], [0., 100., 0.], [f64::NAN, 1., 1.]] {
            assert!(resolve(&request(BASE_FRAME, bad), &snapshot("valid")).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn unknown_or_stale_frames_and_missing_pose_are_rejected() {
        let e = resolve(&request("domain:other", [0., 1., 1.]), &snapshot("valid")).unwrap_err();
        assert!(format!("{e}").contains("known frames: robot.base"), "{e}");
        let e = resolve(&request(SCENE, [9., 2., 0.5]), &snapshot("stale")).unwrap_err();
        assert!(format!("{e}").contains("is stale"), "{e}");
        let mut lost = snapshot("valid");
        lost["registries"]["local_mapping"]["robot_pose"] = Value::Null;
        assert!(resolve(&request(MAP, [-1., 2., 0.5]), &lost).is_err());
        let mut bad_id = request(MAP, [-1., 2., 0.5]);
        bad_id.request_id = "not-a-uuid".into();
        assert!(resolve(&bad_id, &snapshot("valid")).is_err());
    }

    #[test]
    fn busy_rejects_until_hold_and_motion_finish() {
        let mut busy = Busy::default();
        let t = Instant::now();
        busy.claim("a", t).unwrap();
        let e = busy.claim("b", t + Duration::from_secs(1)).unwrap_err();
        assert!(format!("{e}").contains("busy pointing for request a"), "{e}");
        busy.claim("c", t + MOTION_ALLOWANCE + HOLD).unwrap();
    }
}
