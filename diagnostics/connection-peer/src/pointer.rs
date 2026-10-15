//! `point_at` Operable over this peer's own declared frames and transforms.
//! A target frame resolves into the base only through compact pose Products
//! this peer publishes, on their one clock and session; there is no frame-name lookup and
//! no axis guess. With an arm adapter the target then goes through the
//! declared arm mount into the arm IK frame, and the adapter plans and moves;
//! without one the request is resolved and accepted only.
use crate::compact_pose::{Contract, FrameRef, Lifetime, Update};
use crate::point_at::*;
use anyhow::{Context, Result, bail, ensure};
use auki_components::*;
use auki_odometry::{Pose, Tracking};
use auki_registry::{CoordinateConvention, FrameRegistryEntry, LengthUnit};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The arm holds the pointing pose this long before returning.
pub const HOLD: Duration = Duration::from_secs(3);
/// Nominal move-out plus return time, until real motion reports its own.
const MOTION_ALLOWANCE: Duration = Duration::from_secs(8);
/// A sampled transform older than this does not describe the robot now.
const MAX_SAMPLE_AGE_NS: u64 = 800_000_000;
const MIN_DISTANCE_M: f64 = 0.3;
const MAX_DISTANCE_M: f64 = 30.;

pub struct Transform {
    pub contract: Contract,
    pub product: RetainedProduct<Update>,
}

/// The frame definitions and transforms this peer publishes. A withdrawn
/// transform stays listed: its latest observation says it is lost.
#[derive(Default)]
pub struct Frames {
    frames: Vec<FrameRegistryEntry>,
    transforms: Vec<Transform>,
}
impl Frames {
    pub fn insert(&mut self, frames: &[&FrameRegistryEntry], transform: Transform) {
        for f in frames {
            if !self.frames.contains(f) {
                self.frames.push((*f).clone());
            }
        }
        let id = transform.product.producer.output_id.clone();
        self.transforms.retain(|t| t.product.producer.output_id != id);
        self.transforms.push(transform);
    }
    fn definition(&self, f: &FrameRef) -> Option<&FrameRegistryEntry> {
        self.frames.iter().find(|d| FrameRef::from(*d) == *f)
    }
}

fn identity() -> Pose {
    Pose {
        translation: [0.; 3],
        rotation_xyzw: [0., 0., 0., 1.],
    }
}

pub fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

/// The latest observation of `t`, as the destination-from-source transform.
fn value(t: &Transform, clock: &ClockReference, session: &str, now: u64) -> Result<(Pose, Used)> {
    t.contract.validate_output(&t.product.producer)?;
    ensure!(
        t.contract.clock == *clock,
        "{} -> {} is on another clock; crossing clocks needs a time transform",
        t.contract.from_frame.id,
        t.contract.to_frame.id
    );
    // A new session is a new frame lineage; never compose across a reset.
    ensure!(
        t.contract.session_id == session,
        "{} -> {} belongs to another session",
        t.contract.from_frame.id,
        t.contract.to_frame.id
    );
    let o = crate::alignment::latest(&t.product)
        .with_context(|| format!("{} -> {} has no observation", t.contract.from_frame.id, t.contract.to_frame.id))?;
    ensure!(o.output == t.product.producer.reference(), "Transform provenance mismatch");
    o.payload.validate()?;
    let Tracking::Valid { pose } = &o.payload.tracking else {
        bail!("{} -> {} is lost", t.contract.from_frame.id, t.contract.to_frame.id)
    };
    if t.contract.lifetime == Lifetime::Sampled {
        ensure!(
            o.timestamp_ns <= now && now - o.timestamp_ns <= MAX_SAMPLE_AGE_NS,
            "{} -> {} is stale; tracking or pose input stalled",
            t.contract.from_frame.id,
            t.contract.to_frame.id
        );
    }
    let used = Used {
        output: o.output.clone(),
        sequence: o.sequence,
        timestamp_ns: o.timestamp_ns,
    };
    Ok((pose.clone(), used))
}

pub fn through(pose: &Pose, point: [f64; 3]) -> Result<[f64; 3]> {
    let at = Pose {
        translation: point,
        ..identity()
    };
    Ok(crate::alignment::compose(pose, &at)?.translation)
}

/// `to <- from` composed from published transforms, with the observations used.
pub fn chain(
    known: &Frames,
    from: &FrameRef,
    to: &FrameRef,
    clock: &ClockReference,
    session: &str,
    now: u64,
) -> Result<(Pose, Vec<Used>)> {
    // Breadth-first over published transforms, inverting where needed.
    let mut reached = vec![(from.clone(), identity(), Vec::<Used>::new())];
    let mut refused = Vec::new();
    let mut i = 0;
    while i < reached.len() && reached[i].0 != *to {
        let (at, so_far, used) = reached[i].clone();
        for t in &known.transforms {
            let forward = t.contract.from_frame == at;
            if !forward && t.contract.to_frame != at {
                continue;
            }
            let next = if forward { &t.contract.to_frame } else { &t.contract.from_frame };
            if reached.iter().any(|r| r.0 == *next) {
                continue;
            }
            match value(t, clock, session, now) {
                Ok((pose, observation)) => {
                    let step = if forward { pose } else { crate::alignment::inverse(&pose)? };
                    let mut used = used.clone();
                    used.push(observation);
                    reached.push((next.clone(), crate::alignment::compose(&step, &so_far)?, used));
                }
                Err(e) => refused.push(e.to_string()),
            }
        }
        i += 1;
    }
    let Some((_, pose, used)) = reached.into_iter().find(|r| r.0 == *to) else {
        bail!(
            "No usable transform from {} to {}{}",
            from.id,
            to.id,
            if refused.is_empty() { String::new() } else { format!(": {}", refused.join("; ")) }
        )
    };
    Ok((pose, used))
}

/// Resolve `request` into `base` through the transforms in `known`.
pub fn resolve(
    request: &PointAt,
    known: &Frames,
    base: &FrameRef,
    clock: &ClockReference,
    session: &str,
    now: u64,
) -> Result<PointAtAccepted> {
    uuid::Uuid::parse_str(&request.request_id).context("request_id must be a UUID")?;
    ensure!(request.point.iter().all(|v| v.is_finite()), "Point must be finite");
    let listed = || {
        known
            .frames
            .iter()
            .map(|f| f.frame_id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let source = known.definition(&request.frame).with_context(|| {
        format!(
            "{} ({}) is not a frame this robot relates; known frames: {}",
            request.frame.id,
            request.frame.peer_id,
            listed()
        )
    })?;
    let base_definition = known.definition(base).context("Base frame undeclared")?;
    // The arm rules read x as right and y as forward: hold the base to that.
    ensure!(
        *base_definition
            == FrameRegistryEntry::in_convention(
                &base.peer_id,
                &base.id,
                CoordinateConvention::ZUpRightForward
            ),
        "Base frame is not Z-up, X-right, Y-forward"
    );
    ensure!(
        source.units == LengthUnit::Meters && base_definition.units == LengthUnit::Meters,
        "Point frames must be in metres"
    );
    let (base_from_source, resolved_from) = chain(known, &request.frame, base, clock, session, now)?;
    let point_in_base = through(&base_from_source, request.point)?;
    let [x, y, z] = point_in_base;
    let distance = (x * x + y * y + z * z).sqrt();
    ensure!(
        (MIN_DISTANCE_M..=MAX_DISTANCE_M).contains(&distance),
        "Target is {distance:.2} m from the robot; must be {MIN_DISTANCE_M}..{MAX_DISTANCE_M} m"
    );
    ensure!(y > 0., "Target is behind the robot (forward {y:.2} m); turn the robot first");
    let arm = match request.arm {
        ArmChoice::Auto if x > 0. => ArmChoice::Right,
        ArmChoice::Auto => ArmChoice::Left,
        chosen => chosen,
    };
    Ok(PointAtAccepted {
        request_id: request.request_id.clone(),
        arm,
        frame: request.frame.clone(),
        base_frame: base.clone(),
        point_in_base,
        distance_m: distance,
        bearing_deg: (-x).atan2(y).to_degrees(),
        elevation_deg: z.atan2((x * x + y * y).sqrt()).to_degrees(),
        hold_s: HOLD.as_secs_f64(),
        executes: false,
        clock: clock.clone(),
        resolved_from,
    })
}

/// One request at a time; others are rejected until it finishes.
#[derive(Default)]
pub struct Busy {
    active: Option<(String, Instant)>,
}
impl Busy {
    pub fn claim(&mut self, request_id: &str, now: Instant, window: Duration) -> Result<()> {
        if let Some((id, until)) = &self.active
            && now < *until
        {
            bail!(
                "Busy pointing for request {id} ({:.1} s left)",
                (*until - now).as_secs_f64()
            );
        }
        self.active = Some((request_id.into(), now + window));
        Ok(())
    }
    /// End `request_id`'s window early (or any window, for `None`).
    pub fn release(&mut self, request_id: Option<&str>) {
        if request_id.is_none() || self.active.as_ref().is_some_and(|a| Some(a.0.as_str()) == request_id) {
            self.active = None;
        }
    }
}

pub struct Pointer {
    _component: Component,
    _operation: Operable<PointAt, PointAtAccepted>,
    _arm: Option<Arc<crate::arm::Arm>>,
}
impl Pointer {
    /// `arm` is None to resolve only, or Some(execute) to plan with the arm
    /// adapter and, when `execute`, move.
    pub fn mount(p: &crate::poses::Publisher, known: Arc<Mutex<Frames>>, arm: Option<bool>) -> Result<Self> {
        let base: FrameRef = (&p.contract.from_frame).into();
        let clock = p.contract.clock.clone();
        let session = p.contract.session_id.clone();
        let busy = Arc::new(Mutex::new(Busy::default()));
        let arm = match arm {
            Some(execute) => {
                let released = busy.clone();
                let arm = crate::arm::Arm::start(p, execute, move |id| {
                    if let Ok(mut busy) = released.lock() {
                        busy.release(id);
                    }
                })?;
                known.lock().unwrap().insert(&[&arm.frame], crate::pointer::Transform {
                    contract: arm.transform.contract.clone(),
                    product: arm.transform.product.clone(),
                });
                Some(Arc::new(arm))
            }
            None => None,
        };
        let adapter = arm.clone();
        let component = p.runtime.component(ComponentSpec::new(COMPONENT).operable(contract()))?;
        let operation = component.operable_ordered(
            OPERATION,
            InvocationOrdering::SerialInAcceptanceOrder,
            // Any authenticated peer for now; the caller is still checked here.
            |ctx| !ctx.caller_peer_id.is_empty(),
            move |ctx, request: PointAt| {
                let rejected = |e: anyhow::Error| InvocationError::Rejected(format!("{e:#}"));
                let (mut accepted, in_arm) = {
                    let known = known.lock().map_err(|_| InvocationError::TargetUnavailable)?;
                    let now = now_ns();
                    let accepted = resolve(&request, &known, &base, &clock, &session, now).map_err(rejected)?;
                    let in_arm = match &adapter {
                        Some(arm) => {
                            let to = &arm.transform.contract.from_frame;
                            let (arm_from_base, used) =
                                chain(&known, &base, to, &clock, &session, now).map_err(rejected)?;
                            Some((through(&arm_from_base, accepted.point_in_base).map_err(rejected)?, used))
                        }
                        None => None,
                    };
                    (accepted, in_arm)
                };
                let window = if adapter.is_some() { crate::arm::MOTION_TIMEOUT } else { MOTION_ALLOWANCE + HOLD };
                busy.lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?
                    .claim(&request.request_id, Instant::now(), window)
                    .map_err(rejected)?;
                if let (Some(arm), Some((target, used))) = (&adapter, in_arm) {
                    let planned = arm.plan(&request.request_id, accepted.arm, target);
                    let release = || {
                        if let Ok(mut busy) = busy.lock() {
                            busy.release(Some(&request.request_id));
                        }
                    };
                    match planned {
                        Ok(reply) if reply.phase == "planned" => {
                            accepted.executes = reply.executes == Some(true) && arm.executes;
                            if !accepted.executes {
                                release();
                            }
                        }
                        Ok(reply) => {
                            release();
                            return Err(rejected(anyhow::anyhow!(reply.reason.unwrap_or_else(|| "Arm cannot point there".into()))));
                        }
                        Err(e) => {
                            release();
                            return Err(rejected(e));
                        }
                    }
                    accepted.resolved_from.extend(used);
                }
                crate::diagnostic_log!(
                    "point_at {} from {} in {}: {:?} arm, {:.2} m, bearing {:.0}°, elevation {:.0}° ({})",
                    accepted.request_id, ctx.caller_peer_id, accepted.frame.id, accepted.arm,
                    accepted.distance_m, accepted.bearing_deg, accepted.elevation_deg,
                    if accepted.executes { "moving" } else { "not executed" }
                );
                Ok(accepted)
            },
        )?;
        component.expose()?;
        p.endpoint.export_operable(&operation)?;
        Ok(Self {
            _component: component,
            _operation: operation,
            _arm: arm,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 10_000_000_000;
    fn clock() -> ClockReference {
        ClockReference {
            peer_id: "robot".into(),
            id: "capture".into(),
            hash: "a".repeat(32),
        }
    }
    fn yaw(deg: f64, t: [f64; 3]) -> Pose {
        let h = deg.to_radians() / 2.;
        Pose {
            translation: t,
            rotation_xyzw: [0., 0., h.sin(), h.cos()],
        }
    }
    struct Fixture {
        _runtime: ComponentRuntime,
        _components: Vec<Component>,
        outputs: Vec<ConfiguredObservable<Update>>,
        _captures: Vec<BufferProductCapture<Update>>,
        frames: Frames,
        base: FrameRef,
        world: FrameRef,
        scene: FrameRef,
        arm: FrameRef,
    }
    /// base -> world (sampled) and world -> Home scene (held), as the diagnostic publishes them.
    fn fixture() -> Fixture {
        let runtime = ComponentRuntime::new("robot");
        let base = FrameRegistryEntry::in_convention("robot", "bbos-base", CoordinateConvention::ZUpRightForward);
        let world = FrameRegistryEntry::in_convention("robot", "bbos-world-s", CoordinateConvention::ZUpRightForward);
        let scene = FrameRegistryEntry::opengl("home", "scene");
        let arm = FrameRegistryEntry::ros_body("robot", "bbos-arm-base-s");
        let mut fixture = Fixture {
            _runtime: runtime.clone(),
            _components: vec![],
            outputs: vec![],
            _captures: vec![],
            frames: Frames::default(),
            base: (&base).into(),
            world: (&world).into(),
            scene: (&scene).into(),
            arm: (&arm).into(),
        };
        for (name, from, to, lifetime) in [
            ("pose", &base, &world, Lifetime::Sampled),
            ("alignment", &world, &scene, Lifetime::Held),
            ("arm-mount", &arm, &base, Lifetime::Held),
        ] {
            let contract = Contract {
                from_frame: from.into(),
                to_frame: to.into(),
                clock: clock(),
                session_id: "s".into(),
                lifetime,
            };
            let component = runtime
                .component(ComponentSpec::new(name).observable(ObservableContract {
                    name: "transform".into(),
                    datatype: crate::compact_pose::SCHEMA.into(),
                    schema: crate::compact_pose::SCHEMA.into(),
                    access: vec![ObservationAccess::FollowNew],
                    exposure: Exposure::Cluster,
                }))
                .unwrap();
            let output = component
                .configured_observable(
                    ConfiguredObservableSpec::new(
                        "transform",
                        contract.output_id(),
                        clock(),
                        PayloadContract::Structured(StructuredPayloadContract {
                            modality: "pose".into(),
                            datatype: crate::compact_pose::SCHEMA.into(),
                            schema: crate::compact_pose::SCHEMA.into(),
                            observes: name.into(),
                            unit: None,
                        }),
                    )
                    .in_registered_frame(to.clone()),
                )
                .unwrap();
            component.expose().unwrap();
            let capture = runtime
                .capture_buffer(name, &output, BufferLimits::entries(4), |_: &Update| 256)
                .unwrap();
            fixture.frames.insert(&[from, to], Transform { contract, product: capture.product() });
            fixture._components.push(component);
            fixture.outputs.push(output);
            fixture._captures.push(capture);
        }
        fixture
    }
    fn publish(f: &Fixture, i: usize, t: u64, pose: Option<Pose>) {
        let tracking = pose.map_or(Tracking::Lost, |pose| Tracking::Valid { pose });
        f.outputs[i].publish(t, Arc::new(Update { tracking })).unwrap();
    }
    /// Robot 2 m along world +Y, turned 90° left; world sits 10 m along scene +X.
    fn aligned() -> Fixture {
        let f = fixture();
        publish(&f, 0, NOW - 100_000_000, Some(yaw(90., [0., 2., 0.])));
        publish(&f, 1, 1, Some(Pose { translation: [10., 0., 0.], ..identity() }));
        // As diagnostic-arm.py declares it: base turned +90 deg about Z, at axle height.
        let h = std::f64::consts::FRAC_PI_4;
        publish(&f, 2, 0, Some(Pose { translation: [0., 0., 0.0825], rotation_xyzw: [0., 0., h.sin(), h.cos()] }));
        f
    }
    fn request(frame: &FrameRef, point: [f64; 3]) -> PointAt {
        PointAt {
            request_id: uuid::Uuid::new_v4().to_string(),
            frame: frame.clone(),
            point,
            arm: ArmChoice::Auto,
        }
    }
    fn run(f: &Fixture, r: &PointAt) -> Result<PointAtAccepted> {
        resolve(r, &f.frames, &f.base, &clock(), "s", NOW)
    }
    fn close(a: [f64; 3], b: [f64; 3]) {
        assert!(a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9), "{a:?} vs {b:?}");
    }

    #[test]
    fn world_and_scene_points_resolve_through_published_transforms() {
        let f = aligned();
        // Facing world -X, a point 1 m further along it and 0.5 m up is straight ahead.
        let world = run(&f, &request(&f.world, [-1., 2., 0.5])).unwrap();
        close(world.point_in_base, [0., 1., 0.5]);
        assert!(world.bearing_deg.abs() < 1e-9 && (world.elevation_deg - 26.565).abs() < 1e-3);
        assert_eq!(world.resolved_from.len(), 1);
        let scene = run(&f, &request(&f.scene, [9., 2., 0.5])).unwrap();
        close(scene.point_in_base, [0., 1., 0.5]);
        assert_eq!(scene.resolved_from.len(), 2);
        assert_eq!(scene.clock, clock());
        let base = run(&f, &request(&f.base, [0., 1., 0.5])).unwrap();
        assert!(base.resolved_from.is_empty());
    }

    #[test]
    fn the_arm_receives_targets_through_its_declared_mount() {
        let f = aligned();
        let scene = run(&f, &request(&f.scene, [9., 2., 0.5])).unwrap();
        let (arm_from_base, used) = chain(&f.frames, &f.base, &f.arm, &clock(), "s", NOW).unwrap();
        // 1 m ahead of the base is 1 m along arm_base +X, lowered to the axle.
        close(through(&arm_from_base, scene.point_in_base).unwrap(), [1., 0., 0.5 - 0.0825]);
        assert_eq!(used.len(), 1);
        // The scene reaches the arm too, through world, base and the mount.
        let (_, used) = chain(&f.frames, &f.scene, &f.arm, &clock(), "s", NOW).unwrap();
        assert_eq!(used.len(), 3);
    }

    #[test]
    fn arm_side_bearing_and_reach_rules() {
        let f = aligned();
        let left = run(&f, &request(&f.base, [-0.5, 1., 1.])).unwrap();
        assert_eq!(left.arm, ArmChoice::Left);
        assert!(left.bearing_deg > 0.);
        let right = run(&f, &request(&f.base, [0.5, 1., 1.])).unwrap();
        assert_eq!(right.arm, ArmChoice::Right);
        let mut forced = request(&f.base, [0.5, 1., 1.]);
        forced.arm = ArmChoice::Left;
        assert_eq!(run(&f, &forced).unwrap().arm, ArmChoice::Left);
        for bad in [[0., -1., 1.], [0., 0.1, 0.], [0., 100., 0.], [f64::NAN, 1., 1.]] {
            assert!(run(&f, &request(&f.base, bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn unknown_changed_lost_and_stale_frames_are_refused() {
        let f = aligned();
        // Same ID, different definition: the hash differs, so it is another frame.
        let mut changed = f.scene.clone();
        changed.hash = "b".repeat(32);
        let e = run(&f, &request(&changed, [9., 2., 0.5])).unwrap_err();
        assert!(format!("{e}").contains("known frames: bbos-base"), "{e}");
        let mut bad_id = request(&f.world, [-1., 2., 0.5]);
        bad_id.request_id = "not-a-uuid".into();
        assert!(run(&f, &bad_id).is_err());
        // A stale pose blocks every frame but the base itself.
        let stale = fixture();
        publish(&stale, 0, NOW - 2_000_000_000, Some(yaw(90., [0., 2., 0.])));
        let e = run(&stale, &request(&stale.world, [-1., 2., 0.5])).unwrap_err();
        assert!(format!("{e}").contains("stale"), "{e}");
        assert!(run(&stale, &request(&stale.base, [0., 1., 0.5])).is_ok());
        // A withdrawn alignment refuses the scene but not the world.
        publish(&f, 1, 2, None);
        let e = run(&f, &request(&f.scene, [9., 2., 0.5])).unwrap_err();
        assert!(format!("{e}").contains("is lost"), "{e}");
        assert!(run(&f, &request(&f.world, [-1., 2., 0.5])).is_ok());
        // Another clock is never composed without a time transform.
        let mut other = clock();
        other.hash = "c".repeat(32);
        assert!(resolve(&request(&f.world, [-1., 2., 0.5]), &f.frames, &f.base, &other, "s", NOW).is_err());
        // Nor a transform from another session, even on the same clock.
        assert!(resolve(&request(&f.world, [-1., 2., 0.5]), &f.frames, &f.base, &clock(), "reset", NOW).is_err());
    }

    #[test]
    fn busy_rejects_until_hold_and_motion_finish() {
        let mut busy = Busy::default();
        let t = Instant::now();
        let window = MOTION_ALLOWANCE + HOLD;
        busy.claim("a", t, window).unwrap();
        let e = busy.claim("b", t + Duration::from_secs(1), window).unwrap_err();
        assert!(format!("{e}").contains("request a"), "{e}");
        busy.claim("c", t + window, window).unwrap();
        // The adapter ends a move early; another request's end does not.
        busy.release(Some("a"));
        assert!(busy.claim("d", t + window, window).is_err());
        busy.release(Some("c"));
        busy.claim("d", t + window, window).unwrap();
        // The adapter going away frees the arm.
        busy.release(None);
        busy.claim("e", t + window, window).unwrap();
    }
}
