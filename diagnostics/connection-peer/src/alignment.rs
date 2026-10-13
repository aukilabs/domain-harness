//! Registry-bound Bracket-world -> Home transform. No extrapolation or axis guesses.
use crate::compact_pose::{Contract, Lifetime, Update};
use anyhow::{Context, Result, ensure};
use auki_components::*;
use auki_odometry::{Pose, Tracking};
use nalgebra::{Quaternion, UnitQuaternion, Vector3};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const MAX_GAP_NS: u64 = 250_000_000;
fn rotation(p: &Pose) -> UnitQuaternion<f64> {
    let [x, y, z, w] = p.rotation_xyzw;
    UnitQuaternion::new_normalize(Quaternion::new(w, x, y, z))
}
fn pose(t: Vector3<f64>, r: UnitQuaternion<f64>) -> Pose {
    let q = r.quaternion();
    Pose {
        translation: t.into(),
        rotation_xyzw: [q.i, q.j, q.k, q.w],
    }
}
pub fn compose(a: &Pose, b: &Pose) -> Result<Pose> {
    a.validate()?;
    b.validate()?;
    Ok(pose(
        Vector3::from(a.translation) + rotation(a) * Vector3::from(b.translation),
        rotation(a) * rotation(b),
    ))
}
pub fn inverse(p: &Pose) -> Result<Pose> {
    p.validate()?;
    let r = rotation(p).inverse();
    Ok(pose(-(r * Vector3::from(p.translation)), r))
}
pub fn solve(camera_in_home: &Pose, base_in_bracket: &Pose, camera_to_base: &Pose) -> Result<Pose> {
    compose(
        camera_in_home,
        &inverse(&compose(base_in_bracket, camera_to_base)?)?,
    )
}
pub fn latest<T: Clone>(product: &RetainedProduct<T>) -> Option<Observation<T>> {
    let seq = product.buffer().range().last_sequence?;
    product
        .buffer()
        .snapshot(seq, seq)
        .first()
        .map(|e| e.payload.clone())
}
pub fn sampled_at(product: &RetainedProduct<Update>, contract: &Contract, t: u64) -> Result<Pose> {
    contract.validate_output(&product.producer)?;
    ensure!(
        contract.lifetime == Lifetime::Sampled,
        "Expected a sampled pose"
    );
    // An atomic snapshot of the bounded SDK Product, not a second pose stream/cache.
    let samples = product.buffer().snapshot(0, u64::MAX);
    ensure!(
        samples
            .windows(2)
            .all(|w| w[0].timestamp_ns < w[1].timestamp_ns && w[0].sequence + 1 == w[1].sequence),
        "Pose history has a gap/reset"
    );
    let before = samples
        .iter()
        .rev()
        .find(|s| s.timestamp_ns <= t)
        .context("No pose before camera capture")?;
    let after = samples
        .iter()
        .find(|s| s.timestamp_ns >= t)
        .context("Waiting for pose after camera capture; extrapolation forbidden")?;
    for s in [before, after] {
        ensure!(
            s.payload.output == product.producer.reference()
                && s.payload.timestamp_ns == s.timestamp_ns
                && s.payload.sequence == s.sequence,
            "Pose provenance mismatch"
        );
        s.payload.payload.validate()?;
    }
    let (Tracking::Valid { pose: a }, Tracking::Valid { pose: b }) = (
        &before.payload.payload.tracking,
        &after.payload.payload.tracking,
    ) else {
        anyhow::bail!("Tracking lost at capture")
    };
    if before.sequence == after.sequence {
        return Ok(a.clone());
    }
    let gap = after.timestamp_ns - before.timestamp_ns;
    ensure!(gap <= MAX_GAP_NS, "Pose bracket exceeds 250 ms");
    let f = (t - before.timestamp_ns) as f64 / gap as f64;
    let result = pose(
        Vector3::from(a.translation) * (1. - f) + Vector3::from(b.translation) * f,
        rotation(a).slerp(&rotation(b), f),
    );
    result.validate()?;
    Ok(result)
}

pub fn localize(
    home: &crate::home::Verified,
    c: &crate::camera::Calibration,
    cal: &auki_qr_localizer::Calibration,
    d: &crate::camera::Detections,
) -> Result<Pose> {
    c.validate()?;
    // Undistort only the four corners into a virtual pinhole with identical K.
    // R=identity keeps the raw optical frame; the exported extrinsic uses that frame.
    let eye = crate::fisheye::UnrectifiedEye {
        width: c.width,
        height: c.height,
        jpeg_base64: String::new(),
        k: c.k,
        d: c.d,
        r: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
    };
    eye.validate()?;
    let mut candidates = Vec::new();
    for code in &d.codes {
        let Some(anchor) = home.portals.scenegraph.anchors.get(&code.identity) else {
            continue;
        };
        ensure!(
            anchor.payload == code.identity
                || crate::qr_decode::portal_code(&anchor.payload) == Some(code.identity.as_str()),
            "Home portal identity/payload mismatch"
        );
        let Some(corners) = code
            .corners_px
            .iter()
            .map(|p| eye.rectify(*p, c.k))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        if let Ok(estimate) = auki_qr_localizer::estimate_camera_pose(
            &home.portals.scenegraph.map,
            anchor,
            corners.try_into().unwrap(),
            cal,
            Default::default(),
        ) {
            let t = estimate.camera_pose_in_map;
            let [w, x, y, z] = t.rotation_wxyz;
            candidates.push((
                estimate.reprojection_rms_px,
                Pose {
                    translation: t.translation,
                    rotation_xyzw: [x, y, z, w],
                },
            ));
        }
    }
    ensure!(
        !candidates.is_empty(),
        "No known portal passed localization quality checks"
    );
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    let best = &candidates[0].1;
    for (_, other) in &candidates[1..] {
        ensure!(
            (Vector3::from(best.translation) - Vector3::from(other.translation)).norm() < 0.15
                && rotation(best).angle_to(&rotation(other)) < 10f64.to_radians(),
            "Visible Home portals disagree; refusing ambiguous alignment"
        );
    }
    Ok(best.clone())
}
pub struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub target: crate::compact_pose::FrameRef,
}
impl Worker {
    pub fn mount(
        p: &crate::poses::Publisher,
        camera: &crate::camera::Camera,
        home: crate::home::Verified,
        homes: tokio::sync::watch::Receiver<Option<crate::home::Verified>>,
    ) -> Result<Self> {
        let source = p.product();
        let native = Contract {
            from_frame: (&p.contract.from_frame).into(),
            to_frame: (&p.contract.to_frame).into(),
            clock: p.contract.clock.clone(),
            session_id: p.contract.session_id.clone(),
            lifetime: Lifetime::Sampled,
        };
        let contract = Contract {
            from_frame: native.to_frame.clone(),
            to_frame: (&home.frame).into(),
            clock: native.clock.clone(),
            session_id: native.session_id.clone(),
            lifetime: Lifetime::Held,
        };
        let target = contract.to_frame.clone();
        let component = p.runtime.component(
            ComponentSpec::new(format!("bbos-home-alignment-{}", manifest_hash(&target)))
                .observable(ObservableContract {
                    name: "transform".into(),
                    datatype: crate::compact_pose::SCHEMA.into(),
                    schema: crate::compact_pose::SCHEMA.into(),
                    access: vec![ObservationAccess::FollowNew],
                    exposure: Exposure::Cluster,
                }),
        )?;
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "transform",
                contract.output_id(),
                native.clock.clone(),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "pose".into(),
                    datatype: crate::compact_pose::SCHEMA.into(),
                    schema: crate::compact_pose::SCHEMA.into(),
                    observes: "Bracket world in selected Home scene".into(),
                    unit: None,
                }),
            )
            .in_registered_frame(home.frame.clone()),
        )?;
        contract.validate_output(&output.manifest())?;
        component.expose()?;
        let bound = contract.clone();
        let capture = p.runtime.capture_buffer_with_metadata(
            format!("bbos-home-alignment-{}", manifest_hash(&target)),
            &output,
            BufferLimits::entries(64),
            |_: &Update| 256,
            move |o| {
                Ok(Some(CatalogProductMetadata {
                    schema: crate::compact_pose::METADATA.into(),
                    source_sequence: o.sequence,
                    value: serde_json::to_value(&bound).unwrap(),
                }))
            },
        )?;
        p.endpoint.export_product(&capture.product())?;
        let c = camera.calibration();
        let detections = camera.detections.clone();
        let cal = auki_qr_localizer::Calibration::for_product(
            "raw-eye-undistorted-corners",
            &camera.product(),
            auki_qr_localizer::Intrinsics {
                fx: c.k[0],
                fy: c.k[1],
                cx: c.k[2],
                cy: c.k[3],
                distortion: auki_qr_localizer::Distortion::None,
            },
        )?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let (_component, _capture) = (component, capture);
            let mut last_sequence = None;
            let mut last_pose_sequence = None;
            let mut pose_received = Instant::now();
            let mut valid = false;
            let mut published = 0;
            let mut loss = 0;
            let mut geometry = home.identity;
            let mut last_message = String::new();
            while !stopping.load(Ordering::Relaxed) {
                let current = latest(&source);
                if current.as_ref().map(|p| p.sequence) != last_pose_sequence {
                    last_pose_sequence = current.as_ref().map(|p| p.sequence);
                    pose_received = Instant::now();
                }
                for s in source.buffer().snapshot(0, u64::MAX) {
                    if matches!(s.payload.payload.tracking, Tracking::Lost) {
                        loss = loss.max(s.timestamp_ns);
                    }
                }
                let home = homes.borrow().clone();
                let changed = home.as_ref().is_some_and(|h| h.identity != geometry);
                let ready = current
                    .as_ref()
                    .is_some_and(|p| matches!(p.payload.tracking, Tracking::Valid { .. }))
                    && pose_received.elapsed() < Duration::from_millis(800)
                    && home.as_ref().is_some_and(|h| {
                        crate::compact_pose::FrameRef::from(&h.frame) == contract.to_frame
                    });
                if !ready || changed || loss > published {
                    if valid {
                        let at = now_ns().max(published.saturating_add(1));
                        if output
                            .publish(
                                at,
                                Arc::new(Update {
                                    tracking: Tracking::Lost,
                                }),
                            )
                            .is_ok()
                        {
                            published = at;
                            valid = false;
                            loss = loss.max(at);
                            eprintln!(
                                "Alignment withdrawn: tracking, Home contract or connection unavailable"
                            );
                        }
                    }
                    if changed {
                        geometry = home.as_ref().unwrap().identity.clone();
                    }
                    if !ready {
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                }
                if let (Some(home), Some(d)) = (home, latest(&detections)) {
                    // Wait for the right pose bracket, but never reuse pre-loss detections.
                    if last_sequence != Some(d.sequence) && d.timestamp_ns > published.max(loss) {
                        let attempt = (|| -> Result<()> {
                            ensure!(
                                d.output == detections.producer.reference()
                                    && detections.producer.clock == native.clock,
                                "Detection clock/provenance mismatch"
                            );
                            let base = sampled_at(&source, &native, d.timestamp_ns)?;
                            last_sequence = Some(d.sequence); // Retry only missing pose brackets, not expensive rejected PnP.
                            let camera_home = localize(&home, &c, &cal, &d.payload)?;
                            let alignment = solve(&camera_home, &base, &c.camera_to_base)?;
                            output.publish(
                                d.timestamp_ns,
                                Arc::new(Update {
                                    tracking: Tracking::Valid { pose: alignment },
                                }),
                            )?;
                            published = d.timestamp_ns;
                            valid = true;
                            last_sequence = Some(d.sequence);
                            Ok(())
                        })();
                        match attempt {
                            Ok(()) => {
                                if last_message != "aligned" {
                                    eprintln!(
                                        "Alignment established into {}",
                                        contract.to_frame.id
                                    );
                                    last_message = "aligned".into();
                                }
                            }
                            Err(e) => {
                                let message = e.to_string();
                                if message != last_message {
                                    eprintln!("Alignment waiting: {message}");
                                    last_message = message;
                                }
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if valid {
                let _ = output.publish(
                    now_ns().max(published.saturating_add(1)),
                    Arc::new(Update {
                        tracking: Tracking::Lost,
                    }),
                );
            }
        });
        Ok(Self {
            stop,
            thread: Some(thread),
            target,
        })
    }
}
fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn identity() -> Pose {
        Pose {
            translation: [0.; 3],
            rotation_xyzw: [0., 0., 0., 1.],
        }
    }
    fn fixture() -> (
        ComponentRuntime,
        Component,
        ConfiguredObservable<Update>,
        BufferProductCapture<Update>,
        Contract,
    ) {
        let runtime = ComponentRuntime::new("robot");
        let from = auki_registry::FrameRegistryEntry::ros_optical("robot", "base");
        let to = auki_registry::FrameRegistryEntry::ros_optical("robot", "world-session-1");
        let clock = ClockReference {
            peer_id: "robot".into(),
            id: "capture".into(),
            hash: "a".repeat(32),
        };
        let contract = Contract {
            from_frame: (&from).into(),
            to_frame: (&to).into(),
            clock: clock.clone(),
            session_id: "session-1".into(),
            lifetime: Lifetime::Sampled,
        };
        let component = runtime
            .component(ComponentSpec::new("native").observable(ObservableContract {
                name: "pose".into(),
                datatype: crate::compact_pose::SCHEMA.into(),
                schema: crate::compact_pose::SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }))
            .unwrap();
        let output = component
            .configured_observable(
                ConfiguredObservableSpec::new(
                    "pose",
                    contract.output_id(),
                    clock,
                    PayloadContract::Structured(StructuredPayloadContract {
                        modality: "pose".into(),
                        datatype: crate::compact_pose::SCHEMA.into(),
                        schema: crate::compact_pose::SCHEMA.into(),
                        observes: "base".into(),
                        unit: None,
                    }),
                )
                .in_registered_frame(to),
            )
            .unwrap();
        component.expose().unwrap();
        let capture = runtime
            .capture_buffer(
                "history",
                &output,
                BufferLimits::entries(10),
                |_: &Update| 256,
            )
            .unwrap();
        (runtime, component, output, capture, contract)
    }
    #[test]
    fn timestamp_brackets_interpolate_not_latest_and_refuse_loss_gaps_and_reset() {
        let (_r, _c, o, b, c) = fixture();
        let p = b.product();
        let t = 1_000_000_000;
        o.publish(
            t,
            Arc::new(Update {
                tracking: Tracking::Valid { pose: identity() },
            }),
        )
        .unwrap();
        let moved = pose(
            Vector3::new(2., 0., 0.),
            UnitQuaternion::from_axis_angle(&Vector3::z_axis(), std::f64::consts::FRAC_PI_2),
        );
        o.publish(
            t + 100_000_000,
            Arc::new(Update {
                tracking: Tracking::Valid { pose: moved },
            }),
        )
        .unwrap();
        let middle = sampled_at(&p, &c, t + 50_000_000).unwrap();
        assert!((middle.translation[0] - 1.).abs() < 1e-9);
        assert!((rotation(&middle).angle() - std::f64::consts::FRAC_PI_4).abs() < 1e-9);
        assert!(sampled_at(&p, &c, t - 1).is_err());
        assert!(sampled_at(&p, &c, t + 100_000_001).is_err());
        o.publish(
            t + 200_000_000,
            Arc::new(Update {
                tracking: Tracking::Lost,
            }),
        )
        .unwrap();
        o.publish(
            t + 300_000_000,
            Arc::new(Update {
                tracking: Tracking::Valid { pose: identity() },
            }),
        )
        .unwrap();
        assert!(sampled_at(&p, &c, t + 150_000_000).is_err());
        assert!(sampled_at(&p, &c, t + 250_000_000).is_err());
        o.publish(
            t + 900_000_000,
            Arc::new(Update {
                tracking: Tracking::Valid { pose: identity() },
            }),
        )
        .unwrap();
        assert!(sampled_at(&p, &c, t + 500_000_000).is_err());
        let mut reset = c.clone();
        reset.session_id = "new".into();
        assert!(sampled_at(&p, &reset, t).is_err());
        let mut clock = c;
        clock.clock.hash = "b".repeat(64);
        assert!(sampled_at(&p, &clock, t).is_err());
    }
    pub fn scene() -> crate::home::Verified {
        use auki_scenegraph::*;
        let frame = auki_registry::FrameRegistryEntry::opengl("home", "scene");
        let mut graph = Scenegraph::new(MapDefinition {
            map_id: "demo".into(),
            name: None,
            domain_reference: None,
            frame: MapFrame::in_convention(
                "scene",
                "test",
                auki_registry::CoordinateConvention::OpenGl,
                1.,
            )
            .unwrap(),
        })
        .unwrap();
        graph.anchors.insert(
            "280UO1U1RPC".into(),
            QrAnchor {
                anchor_id: "280UO1U1RPC".into(),
                payload: "280UO1U1RPC".into(),
                side_length_m: 0.2,
                pose_in_map: RigidTransform {
                    from_frame_id: "portal".into(),
                    to_frame_id: "scene".into(),
                    translation: [1., 2., 3.],
                    rotation_wxyz: [0.5, 0.5, 0.5, 0.5],
                },
            },
        );
        crate::home::Verified {
            portals: MapSnapshot::new(graph).unwrap(),
            frame,
            identity: "fixture".into(),
        }
    }
    #[test]
    fn raw_fisheye_to_home_chain_preserves_camera_mount_and_native_world() {
        let home = scene();
        let c = crate::camera::Calibration {
            width: 640,
            height: 480,
            k: [500., 500., 320., 240.],
            d: [0.; 4],
            camera_to_base: pose(
                Vector3::new(0.1, 0.2, 0.3),
                UnitQuaternion::from_euler_angles(0.4, -0.2, 0.8),
            ),
        };
        let cal = auki_qr_localizer::Calibration {
            id: "test".into(),
            camera_product: ProductReference {
                peer_id: "robot".into(),
                product_id: "camera".into(),
                manifest_hash: "a".repeat(32),
            },
            camera_frame_id: "optical".into(),
            clock: ClockReference {
                peer_id: "robot".into(),
                id: "capture".into(),
                hash: "a".repeat(32),
            },
            width: 640,
            height: 480,
            fx: 500.,
            fy: 500.,
            cx: 320.,
            cy: 240.,
            distortion: auki_qr_localizer::Distortion::None,
        };
        let corners = [[-0.1f64, -0.1f64], [0.1, -0.1], [0.1, 0.1], [-0.1, 0.1]].map(|[x, y]| {
            let r = x.hypot(y);
            let f = r.atan() / r;
            [500. * x * f + 320., 500. * y * f + 240.]
        });
        let mut d = crate::camera::Detections {
            source_sequence: 10,
            codes: vec![crate::camera::DetectedCode {
                identity: "280UO1U1RPC".into(),
                corners_px: corners,
            }],
            upscaled: false,
        };
        let measured = localize(&home, &c, &cal, &d).unwrap();
        assert!((Vector3::from(measured.translation) - Vector3::new(2., 2., 3.)).norm() < 1e-5);
        let base = pose(
            Vector3::new(3., -2., 1.),
            UnitQuaternion::from_euler_angles(0., 0., 1.2),
        );
        let alignment = solve(&measured, &base, &c.camera_to_base).unwrap();
        let reconstructed =
            compose(&alignment, &compose(&base, &c.camera_to_base).unwrap()).unwrap();
        assert!(
            (Vector3::from(reconstructed.translation) - Vector3::from(measured.translation)).norm()
                < 1e-6
        );
        assert!(rotation(&reconstructed).angle_to(&rotation(&measured)) < 1e-6);
        // Moving in native space uses the same held transform, not a frozen Home pose.
        let moved = compose(
            &base,
            &Pose {
                translation: [1., 0., 0.],
                ..identity()
            },
        )
        .unwrap();
        let before = compose(&alignment, &base).unwrap();
        let after = compose(&alignment, &moved).unwrap();
        assert!(
            ((Vector3::from(after.translation) - Vector3::from(before.translation)).norm() - 1.)
                .abs()
                < 1e-6
        );
        d.codes[0].identity = "UNKNOWN".into();
        assert!(localize(&home, &c, &cal, &d).is_err());
    }
}
