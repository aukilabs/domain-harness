//! One camera Output shared by remote viewers and a bounded local QR worker.
use anyhow::{Result, ensure};
use auki_components::*;
use auki_protocols::registry::v3::RegistryKind;
use auki_registry::{FrameRegistryEntry, SensorRegistryEntry};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub width: u32,
    pub height: u32,
    pub k: [f64; 4],
    pub d: [f64; 4],
    pub camera_to_base: auki_odometry::Pose,
}
impl Calibration {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (64..=2048).contains(&self.width) && (64..=2048).contains(&self.height),
            "Camera dimensions out of bounds"
        );
        ensure!(
            self.k.iter().chain(&self.d).all(|x| x.is_finite()) && self.k[0] > 0. && self.k[1] > 0.,
            "Invalid camera calibration"
        );
        self.camera_to_base.validate()?;
        Ok(())
    }
    fn sensor(&self, frame: &FrameRegistryEntry) -> Result<SensorRegistryEntry> {
        self.validate()?;
        let c = self;
        let sensor: SensorRegistryEntry = serde_json::from_value(
            serde_json::json!({"peer_id":frame.peer_id,"sensor_id":"bbos-head-raw","kind":"camera",
   "type":"mono","width":c.width,"height":c.height,"frame_rate_hz":5,"image_encoding":"jpeg","pixel_format":"gray8","row_stride_bytes":0,
   "color_space":"gray","intrinsics_model":"pinhole","distortion_model":"equidistant",
   "calibration":{"fx":c.k[0],"fy":c.k[1],"cx":c.k[2],"cy":c.k[3],"distortion_coefficients":c.d},
   "frame":{"peer_id":frame.peer_id,"id":frame.frame_id,"hash":frame.hash()}}),
        )?;
        if let auki_registry::SensorBody::Camera(camera) = &sensor.body {
            camera.validate_image_layout()?;
        }
        Ok(sensor)
    }
    fn validate_jpeg(&self, bytes: &[u8]) -> Result<()> {
        ensure!(bytes.len() <= 2 * 1024 * 1024, "Camera frame too large");
        let reader =
            image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg);
        ensure!(
            reader.into_dimensions()? == (self.width, self.height),
            "JPEG dimensions differ from registered calibration"
        );
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Detections {
    pub source_sequence: u64,
    pub codes: Vec<DetectedCode>,
    pub upscaled: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct DetectedCode {
    pub identity: String,
    pub corners_px: [[f64; 2]; 4],
}
impl ContractType for Detections {
    const DATATYPE: &'static str = "spatial-harness.qr-corners/v1";
}
pub struct Camera {
    pub detections: RetainedProduct<Detections>,
    _components: Vec<Component>,
    output: ConfiguredObservable<VideoFrame>,
    _capture: BufferProductCapture<VideoFrame>,
    _extrinsic: BufferProductCapture<crate::compact_pose::Update>,
    tx: Option<tokio::sync::watch::Sender<Option<Observation<VideoFrame>>>>,
    worker: Option<std::thread::JoinHandle<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    calibration: Calibration,
    last: u64,
}
impl Camera {
    pub fn product(&self) -> RetainedProduct<VideoFrame> {
        self._capture.product()
    }
    pub fn calibration(&self) -> Calibration {
        self.calibration.clone()
    }
    /// The localizer's pinned convenience constructor only accepts the legacy
    /// JSON camera schema. Geometry is independent of the transport codec: bind
    /// its public calibration to the real binary Product, never a relabelled one.
    pub fn localization_calibration(&self) -> Result<auki_qr_localizer::Calibration> {
        let product = self.product();
        let output = &product.producer;
        output.validate().map_err(anyhow::Error::msg)?;
        let c = &self.calibration;
        c.validate()?;
        ensure!(matches!(&output.payload, PayloadContract::Camera(v)
            if v.datatype == VideoFrame::DATATYPE && crate::camera_frame::supported(&v.schema)
                && v.width == c.width && v.height == c.height && v.encoding == "jpeg"),
            "Localizer calibration differs from camera Output");
        let frame = output.spatial_frame.as_ref().ok_or_else(||anyhow::anyhow!("Camera frame missing"))?;
        ensure!(frame.definition == FrameRegistryEntry::ros_optical(&output.peer_id, &frame.reference.id),
            "Localizer requires the declared optical camera axes");
        Ok(auki_qr_localizer::Calibration {
            id: "raw-eye-undistorted-corners".into(),
            camera_product: product.reference(), camera_frame_id: frame.reference.id.clone(),
            clock: output.clock.clone(), width: c.width, height: c.height,
            fx: c.k[0], fy: c.k[1], cx: c.k[2], cy: c.k[3],
            // alignment::localize undistorts corners into this same optical frame.
            distortion: auki_qr_localizer::Distortion::None,
        })
    }
    pub fn mount(p: &crate::poses::Publisher, c: Calibration) -> Result<Self> {
        c.validate()?;
        let owner = &p.contract.from_frame.peer_id;
        let frame = FrameRegistryEntry::ros_optical(
            owner,
            format!("bbos-head-raw-{}", p.contract.session_id),
        );
        let sensor = c.sensor(&frame)?;
        let descriptor = serde_json::json!({"sensor":{"peer_id":owner,"id":sensor.sensor_id,"hash":sensor.hash()},
   "frame":crate::compact_pose::FrameRef::from(&frame),"clock":p.contract.clock,"session_id":p.contract.session_id});
        p.definitions.lock().unwrap().extend([
            (
                RegistryKind::Frame,
                frame.frame_id.clone(),
                frame.hash(),
                String::from_utf8(frame.canonical_bytes())?,
            ),
            (
                RegistryKind::Sensor,
                sensor.sensor_id.clone(),
                sensor.hash(),
                String::from_utf8(sensor.canonical_bytes())?,
            ),
        ]);
        let component = p
            .runtime
            .component(
                ComponentSpec::new("bbos-head-camera").observable(ObservableContract {
                    name: "frames".into(),
                    datatype: VideoFrame::DATATYPE.into(),
                    schema: crate::camera_frame::SCHEMA.into(),
                    access: vec![ObservationAccess::FollowNew],
                    exposure: Exposure::Cluster,
                }),
            )?;
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "frames",
                format!("camera-{}", manifest_hash(&descriptor)),
                p.contract.clock.clone(),
                PayloadContract::Camera(CameraPayloadContract {
                    datatype: VideoFrame::DATATYPE.into(),
                    schema: crate::camera_frame::SCHEMA.into(),
                    encoding: "jpeg".into(),
                    width: c.width,
                    height: c.height,
                    nominal_frame_rate_hz: Some(5),
                    observes: "unrectified head left eye".into(),
                }),
            )
            .in_registered_frame(frame.clone()),
        )?;
        component.expose()?;
        let capture = p.runtime.capture_buffer_with_metadata(
            "bbos-head-camera-history",
            &output,
            BufferLimits {
                max_entries: Some(2),
                max_bytes: Some(4 * 1024 * 1024),
                target_duration: None,
            },
            |v: &VideoFrame| v.bytes.len(),
            move |o| {
                Ok(Some(CatalogProductMetadata {
                    schema: "spatial-harness.camera-contract/v1".into(),
                    source_sequence: o.sequence,
                    value: descriptor.clone(),
                }))
            },
        )?;
        p.endpoint.export_product_with_encoder(&capture.product(),crate::camera_frame::encoder())?;
        let contract = crate::compact_pose::Contract {
            from_frame: (&frame).into(),
            to_frame: (&p.contract.from_frame).into(),
            clock: p.contract.clock.clone(),
            session_id: p.contract.session_id.clone(),
            lifetime: crate::compact_pose::Lifetime::Held,
        };
        let extrinsic_component =
            p.runtime
                .component(ComponentSpec::new("bbos-head-extrinsic").observable(
                    ObservableContract {
                        name: "transform".into(),
                        datatype: crate::compact_pose::SCHEMA.into(),
                        schema: crate::compact_pose::SCHEMA.into(),
                        access: vec![ObservationAccess::FollowNew],
                        exposure: Exposure::Cluster,
                    },
                ))?;
        let extrinsic_output = extrinsic_component.configured_observable(
            ConfiguredObservableSpec::new(
                "transform",
                contract.output_id(),
                p.contract.clock.clone(),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "pose".into(),
                    datatype: crate::compact_pose::SCHEMA.into(),
                    schema: crate::compact_pose::SCHEMA.into(),
                    observes: "raw optical camera to base".into(),
                    unit: None,
                }),
            )
            .in_registered_frame(p.contract.from_frame.clone()),
        )?;
        contract.validate_output(&extrinsic_output.manifest())?;
        extrinsic_component.expose()?;
        let extrinsic = p.runtime.capture_buffer_with_metadata(
            "bbos-head-extrinsic",
            &extrinsic_output,
            BufferLimits::entries(1),
            |_: &crate::compact_pose::Update| 256,
            move |o| {
                Ok(Some(CatalogProductMetadata {
                    schema: crate::compact_pose::METADATA.into(),
                    source_sequence: o.sequence,
                    value: serde_json::to_value(&contract).unwrap(),
                }))
            },
        )?;
        p.endpoint.export_product(&extrinsic.product())?;
        // Static mount: valid throughout this capture session, not a moving-head guess.
        extrinsic_output.publish(
            0,
            Arc::new(crate::compact_pose::Update {
                tracking: auki_odometry::Tracking::Valid {
                    pose: c.camera_to_base.clone(),
                },
            }),
        )?;
        let source = output.manifest().reference();
        let detection_contract = serde_json::json!({"source_output":source,"camera_product":capture.product().reference(),"pixel_coordinates":"unrectified source image pixels","frame":crate::compact_pose::FrameRef::from(&frame),"clock":p.contract.clock});
        let detector =
            p.runtime
                .component(ComponentSpec::new("bbos-portal-detector").observable(
                    ObservableContract {
                        name: "detections".into(),
                        datatype: Detections::DATATYPE.into(),
                        schema: Detections::DATATYPE.into(),
                        access: vec![ObservationAccess::FollowNew],
                        exposure: Exposure::Cluster,
                    },
                ))?;
        let detections = detector.configured_observable(
            ConfiguredObservableSpec::new(
                "detections",
                format!("detections-{}", manifest_hash(&detection_contract)),
                p.contract.clock.clone(),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "detection".into(),
                    datatype: Detections::DATATYPE.into(),
                    schema: Detections::DATATYPE.into(),
                    observes: "portal corners".into(),
                    unit: Some("pixels".into()),
                }),
            )
            .in_registered_frame(frame),
        )?;
        detector.expose()?;
        let retained = p.runtime.capture_buffer_with_metadata(
            "bbos-portal-detections",
            &detections,
            BufferLimits::entries(2),
            |_: &Detections| 8192,
            move |o| {
                Ok(Some(CatalogProductMetadata {
                    schema: "spatial-harness.qr-corners-contract/v1".into(),
                    source_sequence: o.sequence,
                    value: detection_contract.clone(),
                }))
            },
        )?;
        p.endpoint.export_product(&retained.product())?;
        let (tx, mut rx) = tokio::sync::watch::channel(None::<Observation<VideoFrame>>);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = stop.clone();
        let detection_product = retained.product();
        let worker = std::thread::spawn(move || {
            let _retained = retained;
            let mut decoder = auki_qr_detector::QrDetector::new(Default::default());
            let mut retry = std::time::Instant::now();
            while !stopping.load(std::sync::atomic::Ordering::Relaxed) {
                if !rx.has_changed().unwrap_or(false) {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    continue;
                }
                let Some(observation) = rx.borrow_and_update().clone() else {
                    continue;
                };
                let began = std::time::Instant::now();
                let result = (|| -> Result<()> {
                    let gray = image::load_from_memory_with_format(
                        &observation.payload.bytes,
                        image::ImageFormat::Jpeg,
                    )?
                    .to_luma8();
                    let allow = began >= retry;
                    if allow {
                        retry = began + std::time::Duration::from_secs(1);
                    }
                    let (codes, upscaled) =
                        crate::qr_decode::detect_codes(&mut decoder, &gray, allow)?;
                    let codes = codes
                        .iter()
                        .filter_map(|c| {
                            c.portal().map(|id| DetectedCode {
                                identity: id.into(),
                                corners_px: c.corners_px,
                            })
                        })
                        .take(32)
                        .collect();
                    detections.publish(
                        observation.timestamp_ns,
                        Arc::new(Detections {
                            source_sequence: observation.sequence,
                            codes,
                            upscaled,
                        }),
                    )?;
                    Ok(())
                })();
                if let Err(e) = result {
                    crate::diagnostic_log!("QR detector: {e}");
                }
                // Bound detector duty cycle without blocking pose or camera delivery.
                let until = std::time::Instant::now() + began.elapsed().saturating_mul(3);
                while std::time::Instant::now() < until
                    && !stopping.load(std::sync::atomic::Ordering::Relaxed)
                {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        });
        Ok(Self {
            detections: detection_product,
            _components: vec![component, extrinsic_component, detector],
            output,
            _capture: capture,
            _extrinsic: extrinsic,
            tx: Some(tx),
            worker: Some(worker),
            stop,
            calibration: c,
            last: 0,
        })
    }
    pub fn publish(&mut self, t: u64, jpeg: Vec<u8>) -> Result<()> {
        self.calibration.validate_jpeg(&jpeg)?;
        if t <= self.last {
            return Ok(());
        }
        let frame = VideoFrame {
            width: self.calibration.width,
            height: self.calibration.height,
            encoding: "jpeg".into(),
            bytes: jpeg.into(),
        };
        let (observation, _) = self.output.publish(t, Arc::new(frame))?;
        self.last = t;
        self.tx.as_ref().unwrap().send_replace(Some(observation));
        Ok(())
    }
}
impl Drop for Camera {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.tx.take();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn calibration() -> Calibration {
        Calibration {
            width: 128,
            height: 128,
            k: [100., 100., 64., 64.],
            d: [0.; 4],
            camera_to_base: auki_odometry::Pose {
                translation: [0.; 3],
                rotation_xyzw: [0., 0., 0., 1.],
            },
        }
    }
    #[test]
    fn camera_registry_pins_owner_frame_and_numeric_calibration() {
        let frame = FrameRegistryEntry::ros_optical("robot", "eye");
        let sensor = calibration().sensor(&frame).unwrap();
        let auki_registry::SensorBody::Camera(camera) = sensor.body else {
            panic!()
        };
        assert_eq!(camera.frame.peer_id, "robot");
        assert_eq!(camera.frame.hash, frame.hash());
        assert_eq!(camera.calibration.unwrap().distortion_coefficients.len(), 4);
    }
    #[test]
    fn rejects_geometry_change_and_invalid_jpeg() {
        let jpeg = include_bytes!("../../../tests/fixtures/bb-lab-portal-280UO1U1RPC-small.jpg");
        let mut c = calibration();
        c.validate_jpeg(jpeg).unwrap();
        c.width = 256;
        assert!(c.validate_jpeg(jpeg).is_err());
        assert!(c.validate_jpeg(b"bad").is_err());
        c.k[0] = f64::NAN;
        assert!(c.validate().is_err());
    }
    #[test]
    fn optimized_retry_detects_small_portal_in_original_pixels() {
        let jpeg = include_bytes!("../../../tests/fixtures/bb-lab-portal-280UO1U1RPC-small.jpg");
        let gray = image::load_from_memory(jpeg).unwrap().to_luma8();
        let mut decoder = auki_qr_detector::QrDetector::new(Default::default());
        assert!(
            crate::qr_decode::detect_codes(&mut decoder, &gray, false)
                .unwrap()
                .0
                .iter()
                .all(|c| c.portal().is_none())
        );
        let (codes, retried) = crate::qr_decode::detect_codes(&mut decoder, &gray, true).unwrap();
        assert!(retried);
        let portal = codes
            .iter()
            .find(|c| c.portal() == Some("280UO1U1RPC"))
            .unwrap();
        assert!((portal.corners_px[0][0] - 72.).abs() < 3.);
        assert!((portal.corners_px[0][1] - 36.).abs() < 3.);
    }
}
