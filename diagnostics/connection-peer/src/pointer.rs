//! `point_at` Operable over this peer's own declared frames and transforms.
//! A target frame resolves into the base only through compact pose Products
//! this peer publishes, on their one clock; there is no frame-name lookup and
//! no axis guess. Milestone 1 resolves and accepts but does not move an arm.
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

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

/// The latest observation of `t`, as the destination-from-source transform.
fn value(t: &Transform, clock: &ClockReference, now: u64) -> Result<(Pose, Used)> {
    t.contract.validate_output(&t.product.producer)?;
    ensure!(
        t.contract.clock == *clock,
        "{} -> {} is on another clock; crossing clocks needs a time transform",
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

fn through(pose: &Pose, point: [f64; 3]) -> Result<[f64; 3]> {
    let at = Pose {
        translation: point,
        ..identity()
    };
    Ok(crate::alignment::compose(pose, &at)?.translation)
}

/// Resolve `request` into `base` through the transforms in `known`.
pub fn resolve(
    request: &PointAt,
    known: &Frames,
    base: &FrameRef,
    clock: &ClockReference,
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
    // Breadth-first over published transforms, inverting where needed.
    let mut reached = vec![(request.frame.clone(), identity(), Vec::<Used>::new())];
    let mut refused = Vec::new();
    let mut i = 0;
    while i < reached.len() && reached[i].0 != *base {
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
            match value(t, clock, now) {
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
    let Some((_, base_from_source, resolved_from)) = reached.into_iter().find(|r| r.0 == *base) else {
        bail!(
            "No usable transform from {} to the robot base{}",
            request.frame.id,
            if refused.is_empty() { String::new() } else { format!(": {}", refused.join("; ")) }
        )
    };
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
    pub fn claim(&mut self, request_id: &str, now: Instant) -> Result<()> {
        if let Some((id, until)) = &self.active
            && now < *until
        {
            bail!(
                "Busy pointing for request {id} ({:.1} s left)",
                (*until - now).as_secs_f64()
            );
        }
        self.active = Some((request_id.into(), now + MOTION_ALLOWANCE + HOLD));
        Ok(())
    }
}

pub struct Pointer {
    _component: Component,
    _operation: Operable<PointAt, PointAtAccepted>,
}
impl Pointer {
    pub fn mount(p: &crate::poses::Publisher, known: Arc<Mutex<Frames>>) -> Result<Self> {
        let base: FrameRef = (&p.contract.from_frame).into();
        let clock = p.contract.clock.clone();
        let component = p.runtime.component(ComponentSpec::new(COMPONENT).operable(contract()))?;
        let busy = Arc::new(Mutex::new(Busy::default()));
        let operation = component.operable_ordered(
            OPERATION,
            InvocationOrdering::SerialInAcceptanceOrder,
            // Any authenticated peer for now; the caller is still checked here.
            |ctx| !ctx.caller_peer_id.is_empty(),
            move |ctx, request: PointAt| {
                let rejected = |e: anyhow::Error| InvocationError::Rejected(format!("{e:#}"));
                let accepted = {
                    let known = known.lock().map_err(|_| InvocationError::TargetUnavailable)?;
                    resolve(&request, &known, &base, &clock, now_ns()).map_err(rejected)?
                };
                busy.lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?
                    .claim(&request.request_id, Instant::now())
                    .map_err(rejected)?;
                crate::diagnostic_log!(
                    "point_at {} from {} in {}: {:?} arm, {:.2} m, bearing {:.0}°, elevation {:.0}° (not executed)",
                    accepted.request_id, ctx.caller_peer_id, accepted.frame.id, accepted.arm,
                    accepted.distance_m, accepted.bearing_deg, accepted.elevation_deg
                );
                Ok(accepted)
            },
        )?;
        component.expose()?;
        p.endpoint.export_operable(&operation)?;
        Ok(Self {
            _component: component,
            _operation: operation,
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
    }
    /// base -> world (sampled) and world -> Home scene (held), as the diagnostic publishes them.
    fn fixture() -> Fixture {
        let runtime = ComponentRuntime::new("robot");
        let base = FrameRegistryEntry::in_convention("robot", "bbos-base", CoordinateConvention::ZUpRightForward);
        let world = FrameRegistryEntry::in_convention("robot", "bbos-world-s", CoordinateConvention::ZUpRightForward);
        let scene = FrameRegistryEntry::opengl("home", "scene");
        let mut fixture = Fixture {
            _runtime: runtime.clone(),
            _components: vec![],
            outputs: vec![],
            _captures: vec![],
            frames: Frames::default(),
            base: (&base).into(),
            world: (&world).into(),
            scene: (&scene).into(),
        };
        for (name, from, to, lifetime) in [
            ("pose", &base, &world, Lifetime::Sampled),
            ("alignment", &world, &scene, Lifetime::Held),
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
        resolve(r, &f.frames, &f.base, &clock(), NOW)
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
        assert!(resolve(&request(&f.world, [-1., 2., 0.5]), &f.frames, &f.base, &other, NOW).is_err());
    }

    #[test]
    fn busy_rejects_until_hold_and_motion_finish() {
        let mut busy = Busy::default();
        let t = Instant::now();
        busy.claim("a", t).unwrap();
        let e = busy.claim("b", t + Duration::from_secs(1)).unwrap_err();
        assert!(format!("{e}").contains("request a"), "{e}");
        busy.claim("c", t + MOTION_ALLOWANCE + HOLD).unwrap();
    }
}
