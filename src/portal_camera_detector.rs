//! Pixel-only Portal detection, independent of tracking and camera calibration.
use crate::{galbot_sensors::SensorSpec, landmarks::*};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// One decode at a time, followed by three times its wall duration idle.
/// This budgets the four camera workers together, without throttling previews.
pub struct DetectorBudget(Mutex<Instant>);
impl Default for DetectorBudget {
    fn default() -> Self { Self(Mutex::new(Instant::now())) }
}

type Pending = Arc<(Mutex<(bool, Option<Observation<VideoFrame>>)>, Condvar)>;
pub struct PortalCameraDetector {
    pending: Pending,
    worker: Option<std::thread::JoinHandle<()>>,
    status: Arc<Mutex<DetectorStatus>>,
}
#[derive(Default, Clone, serde::Serialize)]
pub struct DetectorStatus {
    processed_frames: u64,
    decoded_qrs: u64,
    portal_observations: u64,
    /// Portal observations that only the upscaled retry decoded.
    upscaled_portals: u64,
    errors: u64,
    last_timestamp_ns: Option<u64>,
    last_portal_timestamp_ns: Option<u64>,
    last_portal_landmarks: Vec<String>,
    last_error: Option<String>,
}
impl PortalCameraDetector {
    pub fn mount(
        runtime: &ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        spec: &SensorSpec,
        budget: Arc<DetectorBudget>,
    ) -> anyhow::Result<Self> {
        let (detector, product) = Self::create(runtime, spec, Some(budget))?;
        endpoint.export_product(&product)?;
        Ok(detector)
    }
    fn create(
        runtime: &ComponentRuntime,
        spec: &SensorSpec,
        budget: Option<Arc<DetectorBudget>>,
    ) -> anyhow::Result<(Self, RetainedProduct<LandmarkObservations>)> {
        let id = format!("{}.portal-detector", spec.id);
        let component =
            runtime.component(ComponentSpec::new(&id).observable(ObservableContract {
                name: "landmarks".into(),
                datatype: OBSERVATIONS_SCHEMA.into(),
                schema: OBSERVATIONS_SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }))?;
        let output = component.configured_observable::<LandmarkObservations>(
            ConfiguredObservableSpec::new(
                "landmarks",
                uuid::Uuid::new_v4().to_string(),
                spec.clock.clone(),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "detection".into(),
                    observes: "Portal corners in source camera pixels".into(),
                    unit: Some("pixels".into()),
                    datatype: OBSERVATIONS_SCHEMA.into(),
                    schema: OBSERVATIONS_SCHEMA.into(),
                }),
            ),
        )?;
        component.expose()?;
        let capture = runtime.capture_buffer(
            &format!("{id}-history"),
            &output,
            BufferLimits::entries(2),
            |v: &LandmarkObservations| serde_json::to_vec(v).map_or(0, |v| v.len()),
        )?;
        let product = capture.product();
        let status = Arc::new(Mutex::new(DetectorStatus::default()));
        let worker_status = status.clone();
        let pending: Pending = Arc::new((Mutex::new((false, None)), Condvar::new()));
        let queue = pending.clone();
        let clock = spec.clock.clone();
        let worker = std::thread::spawn(move || {
            let _capture = capture;
            let mut detector = auki_qr_detector::QrDetector::new(Default::default());
            let mut last = None;
            let mut next_camera = Instant::now();
            loop {
                let (lock, ready) = &*queue;
                let mut state = lock.lock().unwrap();
                while !state.0 && state.1.is_none() {
                    state = ready.wait(state).unwrap();
                }
                if state.0 {
                    break;
                }
                // Wait interruptibly and leave the latest-only mailbox intact.
                // A per-camera interval also prevents a fast wrist worker from
                // continuously taking the shared budget from another camera.
                let mut permit = None;
                if let Some(budget) = &budget {
                    if Instant::now() >= next_camera {
                        if let Ok(slot) = budget.0.try_lock() {
                            if Instant::now() >= *slot { permit = Some(slot); }
                        }
                    }
                    if permit.is_none() {
                        let _ = ready.wait_timeout(state, Duration::from_millis(50)).unwrap();
                        continue;
                    }
                }
                let observation = state.1.take().unwrap();
                drop(state);
                let started = Instant::now();
                let result = (|| -> anyhow::Result<()> {
                    let Detection { decoded, landmarks, upscaled } =
                        detect(&mut detector, &observation.payload.bytes)?;
                    let identities = landmarks
                        .iter()
                        .map(|l| l.recognition.identity.clone())
                        .collect::<Vec<_>>();
                    output.publish(
                        observation.timestamp_ns,
                        Arc::new(LandmarkObservations {
                            schema: OBSERVATIONS_SCHEMA.into(),
                            detected_by: component.reference().clone(),
                            source_observation: SourceObservation {
                                output: observation.output,
                                sequence: observation.sequence,
                                timestamp_ns: observation.timestamp_ns,
                                clock: clock.clone(),
                            },
                            landmarks,
                        }),
                    )?;
                    last = Some(observation.timestamp_ns);
                    let mut status = worker_status.lock().unwrap();
                    status.processed_frames += 1;
                    status.decoded_qrs += decoded as u64;
                    status.portal_observations += (identities.len() / 4) as u64;
                    status.upscaled_portals += if upscaled { (identities.len() / 4) as u64 } else { 0 };
                    status.last_timestamp_ns = last;
                    status.last_error = None;
                    if !identities.is_empty() {
                        status.last_portal_timestamp_ns = last;
                        status.last_portal_landmarks = identities;
                    }
                    Ok(())
                })();
                if let Some(mut slot) = permit {
                    *slot = Instant::now() + started.elapsed().saturating_mul(3)
                        .max(Duration::from_millis(250));
                    next_camera = Instant::now() + Duration::from_secs(1);
                }
                if let Err(e) = result {
                    let mut status = worker_status.lock().unwrap();
                    status.errors += 1;
                    status.last_error = Some(e.to_string());
                    eprintln!("{id}: {e}");
                }
            }
            if let Some(t) = last {
                let _ = output.end(t, ObservationEndReason::Reconfigured { replacement: None });
            }
        });
        Ok((
            Self {
                pending,
                worker: Some(worker),
                status,
            },
            product,
        ))
    }
    pub fn status(&self) -> DetectorStatus {
        self.status.lock().unwrap().clone()
    }
    pub fn submit(&self, observation: Observation<VideoFrame>) {
        // Keep only the newest frame; detection never blocks camera publishing.
        self.pending.0.lock().unwrap().1 = Some(observation);
        self.pending.1.notify_one();
    }
}
/// Retry scale for frames whose Portals do not decode at native resolution.
/// A 15 cm Portal a few metres from a 1280-px head eye spans under 2 px per
/// module and the scanner's own small-code rungs do not fire in cluttered
/// scenes. On live BB Lab frames 2× decoded 14 of 24 eye images where 1.5×
/// managed 5 and native none.
pub const UPSCALE_RETRY: Upscale = Upscale::X2;

/// Catmull-Rom upscales with fixed integer taps. Output pixel o samples
/// source position (o+0.5)/f-0.5, so the tap pattern repeats every period.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Upscale {
    X1_5,
    X2,
}
impl Upscale {
    pub fn factor(self) -> f64 {
        match self {
            Upscale::X1_5 => 1.5,
            Upscale::X2 => 2.,
        }
    }
    /// (outputs per period, source pixels per period, per-phase (first tap
    /// offset, weights), weight sum).
    fn kernel(self) -> (usize, usize, &'static [(isize, [i32; 4])], i32) {
        match self {
            // Phases sample t=2/3 from 2k-1, t=1/3 from 2k, and 2k+1 exactly.
            Upscale::X1_5 => (3, 2, &[(-2, [-1, 9, 21, -2]), (-1, [-2, 21, 9, -1]), (1, [27, 0, 0, 0])], 27),
            // Phases sample t=3/4 from k-1 and t=1/4 from k.
            Upscale::X2 => (2, 1, &[(-2, [-3, 29, 111, -9]), (-1, [-9, 111, 29, -3])], 128),
        }
    }
}

/// Separable integer upscale; ~5× faster than the generic resizer, which also
/// clips the intermediate pass.
pub fn upscale(gray: &image::GrayImage, scale: Upscale) -> image::GrayImage {
    let (period, step, phases, sum) = scale.kernel();
    let taps = |o: usize, len: usize| -> [(usize, i32); 4] {
        let (offset, weights) = phases[o%period];
        let first = ((o/period)*step) as isize+offset;
        std::array::from_fn(|i| ((first+i as isize).clamp(0, len as isize-1) as usize, weights[i]))
    };
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let (ow, oh) = (w*period/step, h*period/step);
    let src = gray.as_raw();
    let columns: Vec<_> = (0..ow).map(|o| taps(o, w)).collect();
    let mut rows = vec![0i32; ow*h]; // horizontal pass, scaled by `sum`
    for y in 0..h {
        let row = &src[y*w..(y+1)*w];
        for (out, t) in rows[y*ow..(y+1)*ow].iter_mut().zip(&columns) {
            *out = t.iter().map(|&(x, wt)| wt*i32::from(row[x])).sum();
        }
    }
    let (norm, half) = (sum*sum, sum*sum/2);
    let mut out = vec![0u8; ow*oh];
    for o in 0..oh {
        let t = taps(o, h);
        for (x, px) in out[o*ow..(o+1)*ow].iter_mut().enumerate() {
            let v: i32 = t.iter().map(|&(y, wt)| wt*rows[y*ow+x]).sum();
            *px = ((v+half).div_euclid(norm)).clamp(0, 255) as u8;
        }
    }
    image::GrayImage::from_raw(ow as u32, oh as u32, out).expect("sized buffer")
}

fn detect(
    detector: &mut auki_qr_detector::QrDetector,
    jpeg: &[u8],
) -> anyhow::Result<Detection> {
    let gray = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)?.to_luma8();
    detect_portals(detector, &gray)
}

pub struct Detection {
    pub decoded: usize,
    pub landmarks: Vec<ImageLandmark>,
    /// Found only on the upscaled retry; pixels are mapped back to `gray`.
    pub upscaled: bool,
}

/// One decoded QR code with corners in the scanned image's own pixels.
#[derive(Clone, Debug)]
pub struct Code {
    pub payload: String,
    pub mirrored: bool,
    /// Top-left, top-right, bottom-right, bottom-left (refined when available).
    pub corners_px: [[f64; 2]; 4],
}
impl Code {
    pub fn portal(&self) -> Option<&str> {
        (!self.mirrored).then(|| crate::local_mapper::portal_code(&self.payload)).flatten()
    }
}

/// Every QR code in `gray`. When `retry` is set and no Portal decodes at
/// native resolution, scans once more on an [`UPSCALE_RETRY`] copy and keeps
/// that result if it holds a Portal. Returns (codes, decoded on the retry).
pub fn detect_codes(
    detector: &mut auki_qr_detector::QrDetector,
    gray: &image::GrayImage,
    retry: bool,
) -> anyhow::Result<(Vec<Code>, bool)> {
    let native = scan(detector, gray, 1.)?;
    if !retry || native.iter().any(|c| c.portal().is_some()) {
        return Ok((native, false));
    }
    let larger = upscale(gray, UPSCALE_RETRY);
    let retried = scan(detector, &larger, UPSCALE_RETRY.factor())?;
    Ok(if retried.iter().any(|c| c.portal().is_some()) { (retried, true) } else { (native, false) })
}

/// Portal corners in `gray`'s pixels, with the upscaled retry.
pub fn detect_portals(
    detector: &mut auki_qr_detector::QrDetector,
    gray: &image::GrayImage,
) -> anyhow::Result<Detection> {
    let (codes, upscaled) = detect_codes(detector, gray, true)?;
    let landmarks = codes
        .iter()
        .filter_map(|c| c.portal().map(|id| (id, c.corners_px)))
        .flat_map(|(id, corners)| {
            CORNERS.into_iter().zip(corners).map(move |(corner, pixel)| ImageLandmark {
                recognition: Recognition {
                    landmark_type: PORTAL.into(),
                    identity: format!("{id}/{corner}"),
                },
                pixel,
            })
        })
        .collect();
    Ok(Detection { decoded: codes.len(), landmarks, upscaled })
}

fn scan(
    detector: &mut auki_qr_detector::QrDetector,
    gray: &image::GrayImage,
    scale: f64,
) -> anyhow::Result<Vec<Code>> {
    let found = detector.detect_luma(
        gray.as_raw(),
        gray.width() as usize,
        gray.height() as usize,
        gray.width() as usize,
    )?;
    // Pixel centres: x_source = (x_scaled + 0.5) / scale - 0.5.
    let source = |v: f64| (v+0.5)/scale-0.5;
    Ok(found
        .codes
        .iter()
        .map(|c| Code {
            payload: c.payload.clone(),
            mirrored: c.mirrored,
            corners_px: c
                .refined_corners_px
                .as_ref()
                .unwrap_or(&c.corners_px)
                .map(|p| [source(p.x), source(p.y)]),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn head_detectors_publish_with_no_odometry_or_depth() -> anyhow::Result<()> {
        for id in ["camera.head", "camera.head.right"] {
            let runtime = ComponentRuntime::new("robot");
            let clock = crate::model::clock_ref(&crate::model::clock_entry("robot", "test"));
            let payload = PayloadContract::Camera(CameraPayloadContract {
                datatype: "video_frame".into(),
                schema: "auki.video-frame/v1".into(),
                encoding: "jpeg".into(),
                width: 640,
                height: 480,
                nominal_frame_rate_hz: Some(5),
                observes: id.into(),
            });
            let spec = SensorSpec {
                id: id.into(),
                clock: clock.clone(),
                frame_definition: None,
                payload: payload.clone(),
                metadata: serde_json::json!({}),
            };
            let source =
                runtime.component(ComponentSpec::new(id).observable(ObservableContract {
                    name: "frames".into(),
                    datatype: "video_frame".into(),
                    schema: "auki.video-frame/v1".into(),
                    access: vec![ObservationAccess::FollowNew],
                    exposure: Exposure::Cluster,
                }))?;
            let output = source.configured_observable::<VideoFrame>(
                ConfiguredObservableSpec::new("frames", "test", clock.clone(), payload)
                    .in_registered_frame(auki_registry::FrameRegistryEntry::ros_optical("robot", id)),
            )?;
            source.expose()?;
            let budget = Arc::new(DetectorBudget::default());
            let (detector, product) = PortalCameraDetector::create(&runtime, &spec, Some(budget.clone()))?;
            for (t, data) in [
                (1, jpeg(Some("https://r8.hr/2ZWHV5L6I04"))),
                (2, jpeg(None)),
            ] {
                let (observation, _) = output.publish(
                    t,
                    Arc::new(VideoFrame {
                        width: 640,
                        height: 480,
                        encoding: "jpeg".into(),
                        bytes: data.into(),
                    }),
                )?;
                let sequence = observation.sequence;
                // Hold the shared slot unavailable: submitting must remain
                // nonblocking, and no decode may run before the budget opens.
                *budget.0.lock().unwrap() = Instant::now() + Duration::from_millis(150);
                let before = detector.status().processed_frames;
                detector.submit(observation);
                std::thread::sleep(Duration::from_millis(75));
                assert_eq!(detector.status().processed_frames, before);
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while detector.status().last_timestamp_ns != Some(t) {
                    anyhow::ensure!(
                        std::time::Instant::now() < deadline,
                        "detector did not publish"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                let batch = product.latest_existing()?.unwrap();
                assert_eq!(batch.payload.source_observation.output.component_id, id);
                assert_eq!(batch.payload.source_observation.sequence, sequence);
                assert_eq!(batch.payload.source_observation.timestamp_ns, t);
                assert_eq!(batch.payload.source_observation.clock, clock);
                assert_eq!(batch.payload.landmarks.len(), if t == 1 { 4 } else { 0 });
            }
            let status = detector.status();
            assert_eq!(status.processed_frames, 2);
            assert_eq!(status.decoded_qrs, 1);
            assert_eq!(status.portal_observations, 1);
            assert_eq!(status.last_portal_timestamp_ns, Some(1));
            assert_eq!(status.errors, 0);
        }
        Ok(())
    }
    fn jpeg(payload: Option<&str>) -> Vec<u8> {
        let mut pixels = image::GrayImage::from_pixel(640, 480, image::Luma([255]));
        if let Some(payload) = payload {
            let qr = qrcode::QrCode::new(payload).unwrap();
            for y in 0..qr.width() {
                for x in 0..qr.width() {
                    if qr[(x, y)] == qrcode::Color::Dark {
                        for dy in 0..8 {
                            for dx in 0..8 {
                                pixels.put_pixel(
                                    (100 + x * 8 + dx) as u32,
                                    (80 + y * 8 + dy) as u32,
                                    image::Luma([0]),
                                );
                            }
                        }
                    }
                }
            }
        }
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
            .encode_image(&pixels)
            .unwrap();
        bytes
    }
    #[test]
    fn upscale_keeps_flat_images_flat_and_sizes_exact() {
        let flat = image::GrayImage::from_pixel(7, 5, image::Luma([123]));
        for (scale, (w, h)) in [(Upscale::X1_5, (10, 7)), (Upscale::X2, (14, 10))] {
            let up = upscale(&flat, scale);
            assert_eq!((up.width(), up.height()), (w, h));
            assert!(up.pixels().all(|p| p.0[0] == 123));
        }
    }
    #[test]
    fn small_portal_decodes_only_on_the_upscaled_retry_in_source_pixels() {
        // 128×128 crop of a live BB Lab head-eye frame: the 15 cm Portal at
        // about 1.5 px per module, which the native pass cannot read.
        let jpeg = include_bytes!("../tests/fixtures/bb-lab-portal-280UO1U1RPC-small.jpg");
        let gray = image::load_from_memory(jpeg).unwrap().to_luma8();
        let mut detector = auki_qr_detector::QrDetector::new(Default::default());
        assert!(scan(&mut detector, &gray, 1.).unwrap().iter().all(|c| c.portal().is_none()), "fixture must fail natively");
        let found = detect(&mut detector, jpeg).unwrap();
        assert!(found.upscaled);
        assert_eq!(found.landmarks.len(), 4);
        assert_eq!(found.landmarks[0].recognition.identity, "280UO1U1RPC/top_left");
        // Corners are reported in the crop's own pixels, not the upscaled copy's.
        let tl = found.landmarks[0].pixel;
        assert!((tl[0]-72.).abs() < 3. && (tl[1]-36.).abs() < 3., "{tl:?}");
    }
    #[test]
    fn wrist_jpeg_yields_four_named_corners_without_calibration() {
        let mut detector = auki_qr_detector::QrDetector::new(Default::default());
        let Detection { decoded, landmarks: found, upscaled } =
            detect(&mut detector, &jpeg(Some("https://r8.hr/2ZWHV5L6I04"))).unwrap();
        assert_eq!(decoded, 1);
        assert!(!upscaled);
        assert_eq!(found.len(), 4);
        for (landmark, corner) in found.iter().zip(CORNERS) {
            assert_eq!(
                landmark.recognition.identity,
                format!("2ZWHV5L6I04/{corner}")
            );
            assert_eq!(landmark.recognition.landmark_type, PORTAL);
            assert!(landmark.pixel.iter().all(|p| p.is_finite()));
        }
        assert!(detect(&mut detector, &jpeg(None)).unwrap().landmarks.is_empty());
        assert!(
            detect(&mut detector, &jpeg(Some("not a portal")))
                .unwrap()
                .landmarks
                .is_empty()
        );
        assert!(detect(&mut detector, b"invalid jpeg").is_err());
    }
}
impl Drop for PortalCameraDetector {
    fn drop(&mut self) {
        self.pending.0.lock().unwrap().0 = true;
        self.pending.1.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
