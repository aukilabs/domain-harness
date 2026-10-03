//! Same-module extension: one Python AukiPeer, real SDK Components and protocols.
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::useless_conversion)]
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;
use auki_datatypes::pose::{Quat, SpatialTransform, Vec3};
use auki_domain_client::{DataError, Portal, PortalId};
use auki_geometry::pnp::{Camera, Vector2, estimate_square_pose_from_pixels, pose_tools};
use auki_portal_detector::{PortalDetection, PortalDetector, PortalSize};
use auki_qr_mapper::{
    PortalSizeError, ResolvedPortal,
    local::{AnchorPlacement, EnsureQrResult, PortalMapper, PortalMetadataSource},
};
use auki_scenegraph::{component::*, resolution::PortalMaps, *};
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use serde::Deserialize;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod odometry;
mod sensors;
use auki_qr_localizer::component::{LocalizeOnce, QrLocalizerComponent};
use auki_qr_localizer::{Calibration, Distortion, Intrinsics, QualityGate};

type Result<T> = std::result::Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn ticks() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("UTC clock")
        .as_nanos() as u64
}

#[derive(Clone)]
struct Metadata(
    Arc<Py<PyAny>>,
    Arc<Mutex<std::collections::BTreeMap<String, ResolvedPortal>>>,
);
impl Metadata {
    fn new(callback: Py<PyAny>) -> Self {
        Self(
            Arc::new(callback),
            Arc::new(Mutex::new(std::collections::BTreeMap::new())),
        )
    }
}
impl PortalMetadataSource for Metadata {
    async fn resolve(
        &self,
        domain: Uuid,
        portal: &PortalId,
        cancel: &CancellationToken,
    ) -> std::result::Result<ResolvedPortal, PortalSizeError> {
        if cancel.is_cancelled() {
            return Err(DataError::Cancelled.into());
        }
        let key = format!("{domain}/{}", portal.as_str());
        if let Some(value) = self.1.lock().unwrap().get(&key).cloned() {
            return Ok(value);
        }
        let raw = Python::with_gil(|py| {
            self.0
                .call1(py, (domain.to_string(), portal.as_str()))?
                .extract::<String>(py)
        })
        .map_err(|_| PortalSizeError::from(DataError::Callback))?;
        if cancel.is_cancelled() {
            return Err(DataError::Cancelled.into());
        }
        let record: Portal = serde_json::from_str(&raw)
            .map_err(|_| DataError::InvalidResponse("Portal metadata"))?;
        if !portal.matches(record.id, &record.short_id) {
            return Err(DataError::InvalidResponse("Portal identity").into());
        }
        let resolved = ResolvedPortal::try_from(record)?;
        let mut cache = self.1.lock().unwrap();
        if cache.len() >= 128 {
            cache.pop_first();
        }
        cache.insert(key, resolved.clone());
        Ok(resolved)
    }
}

#[derive(Deserialize, Clone)]
struct Config {
    domain_id: Uuid,
    map_id: String,
    map_frame: String,
    optical_frame: String,
    optical_registry: FrameRegistryEntry,
    capture_clock: ClockReference,
    publication_clock: ClockReference,
    width: u32,
    height: u32,
    fx: f64,
    fy: f64,
    cx: f64,
    cy: f64,
}

struct Engine {
    runtime: ComponentRuntime,
    endpoint: Option<ComponentProtocolEndpoint>,
    _camera: Component,
    capture: BufferProductCapture<VideoFrame>,
    frames: ConfiguredObservable<VideoFrame>,
    detector: PortalDetector,
    results: BufferProductCapture<PortalDetection>,
    maps: PortalMaps,
    reference_map: Arc<MapComponent>,
    localizer: QrLocalizerComponent,
    map: Arc<MapComponent>,
    metadata: Metadata,
    cfg: Config,
    rt: tokio::runtime::Runtime,
    last_timestamp: Option<u64>,
}
impl Engine {
    fn new(
        peer_id: &str,
        protocols: ::auki_sdk::AukiPeerProtocols,
        cfg: Config,
        metadata: Metadata,
    ) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new().map_err(err)?;
        let entered = rt.enter();
        let runtime = ComponentRuntime::new(peer_id.to_string());
        let endpoint = ComponentProtocolEndpoint::mount(protocols, runtime.clone()).map_err(err)?;
        let mut engine = Self::in_host(runtime, &endpoint, cfg, metadata)?;
        engine.endpoint = Some(endpoint);
        drop(entered);
        // Keep the runtime that mounted the endpoint alive.
        engine.rt = rt;
        Ok(engine)
    }
    fn in_host(
        runtime: ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        cfg: Config,
        metadata: Metadata,
    ) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new().map_err(err)?;
        let entered = rt.enter();
        let peer_id = runtime.peer_id();
        if cfg.width == 0
            || cfg.height == 0
            || u64::from(cfg.width) * u64::from(cfg.height) > 16_777_216
        {
            return Err("camera dimensions out of bounds".into());
        }
        Camera::pinhole(cfg.fx, cfg.fy, cfg.cx, cfg.cy).map_err(err)?;
        let context = InvocationContext {
            invocation_id: Uuid::new_v4().to_string(),
            caller_peer_id: peer_id.into(),
            caller_component_id: "mapping-host".into(),
        };
        let owner = peer_id.to_string();
        let map = Arc::new(
            MapComponent::new(
                &runtime,
                MapComponentConfig {
                    component_id: "portal-map".into(),
                    publication_id: Uuid::new_v4().to_string(),
                    clock: cfg.publication_clock.clone(),
                    map: MapDefinition {
                        map_id: cfg.map_id.clone(),
                        name: Some("Observed Galbot Portals".into()),
                        domain_reference: Some(cfg.domain_id.to_string()),
                        frame: MapFrame::in_convention(
                            &cfg.map_frame,
                            "Legacy calibration pinned; explicit OpenGL to Z-up conversion",
                            CoordinateConvention::ZUpRightForward,
                            1.,
                        )
                        .map_err(err)?,
                    },
                },
                ticks,
                |_| true,
                move |c| c.caller_peer_id == owner && c.caller_component_id == "mapping-host",
            )
            .map_err(err)?,
        );
        let maps = PortalMaps::new(context);
        maps.register(map.clone()).map_err(err)?;
        let reference_map = Arc::new(
            MapComponent::new(
                &runtime,
                MapComponentConfig {
                    component_id: "legacy-reference-map".into(),
                    publication_id: Uuid::new_v4().to_string(),
                    clock: cfg.publication_clock.clone(),
                    map: MapDefinition {
                        map_id: format!("{}-reference", cfg.map_id),
                        name: Some("Legacy portal reference".into()),
                        domain_reference: Some(cfg.domain_id.to_string()),
                        frame: MapFrame::in_convention(
                            &cfg.map_frame,
                            "Legacy domain converted to Z up",
                            CoordinateConvention::ZUpRightForward,
                            1.,
                        )
                        .map_err(err)?,
                    },
                },
                ticks,
                |_| true,
                |c| c.caller_component_id == "mapping-host",
            )
            .map_err(err)?,
        );
        maps.register(reference_map.clone()).map_err(err)?;
        if cfg.optical_registry.frame_id != cfg.optical_frame {
            return Err("optical frame registry does not match mapping calibration".into());
        }
        let camera = runtime
            .component(
                ComponentSpec::new("mapping-camera").observable(ObservableContract {
                    name: "frames".into(),
                    datatype: VideoFrame::DATATYPE.into(),
                    schema: "auki.video-frame/v1".into(),
                    access: vec![ObservationAccess::FollowNew],
                    exposure: Exposure::Cluster,
                }),
            )
            .map_err(err)?;
        let frames = camera
            .configured_observable(
                ConfiguredObservableSpec::new(
                    "frames",
                    Uuid::new_v4().to_string(),
                    cfg.capture_clock.clone(),
                    PayloadContract::Camera(CameraPayloadContract {
                        datatype: VideoFrame::DATATYPE.into(),
                        schema: "auki.video-frame/v1".into(),
                        encoding: "jpeg".into(),
                        width: cfg.width,
                        height: cfg.height,
                        nominal_frame_rate_hz: None,
                        observes: "Galbot optical camera".into(),
                    }),
                )
                .in_registered_frame(cfg.optical_registry.clone()),
            )
            .map_err(err)?;
        camera.expose().map_err(err)?;
        let capture = runtime
            .capture_buffer(
                "mapping-camera-frames",
                &frames,
                sensors::history_limits(),
                |v: &VideoFrame| v.bytes.len(),
            )
            .map_err(err)?;
        let detector = PortalDetector::bind(
            &runtime,
            "portal-detector",
            &Uuid::new_v4().to_string(),
            &capture.product(),
            maps.clone(),
            metadata.clone(),
            cfg.domain_id,
            cfg.publication_clock.clone(),
            ticks,
        )?;
        let results = runtime
            .capture_buffer(
                "portal-detections",
                detector.portals(),
                sensors::history_limits(),
                |_| 16384,
            )
            .map_err(err)?;
        let localizer = QrLocalizerComponent::bind(
            &runtime,
            "portal-localizer",
            Uuid::new_v4().to_string(),
            &detector.detection_product(),
            Arc::new(maps.clone()),
            Calibration::for_product(
                "galbot-rectified",
                &capture.product(),
                Intrinsics {
                    fx: cfg.fx,
                    fy: cfg.fy,
                    cx: cfg.cx,
                    cy: cfg.cy,
                    distortion: Distortion::None,
                },
            )
            .map_err(err)?,
            QualityGate::default(),
        )
        .map_err(err)?;
        endpoint
            .export_product(&reference_map.product())
            .map_err(err)?;
        endpoint
            .export_operable(reference_map.lookup_qr())
            .map_err(err)?;
        endpoint
            .export_operable(reference_map.find_qr())
            .map_err(err)?;
        endpoint.export_product(&map.product()).map_err(err)?;
        endpoint.export_product(&capture.product()).map_err(err)?;
        endpoint
            .export_product(&detector.detection_product())
            .map_err(err)?;
        endpoint.export_product(&results.product()).map_err(err)?;
        endpoint.export_operable(map.lookup_qr()).map_err(err)?;
        endpoint.export_operable(map.find_qr()).map_err(err)?;
        // No remote write Operable is exported. Local mapper admission remains authorized.
        drop(entered);
        Ok(Self {
            runtime,
            endpoint: None,
            _camera: camera,
            capture,
            frames,
            detector,
            results,
            maps,
            reference_map,
            localizer,
            map,
            metadata,
            cfg,
            rt,
            last_timestamp: None,
        })
    }
    fn snapshot(&self) -> Result<String> {
        let observation = self
            .map
            .product()
            .latest_existing()
            .map_err(err)?
            .ok_or("missing map snapshot")?;
        let snapshot = &observation.payload;
        serde_json::to_string(&serde_json::json!({"snapshot_reference":self.map.snapshot_reference(), "snapshot":snapshot,
            "inspection_usda":snapshot.scenegraph.to_usda_with_portal_geometry().map_err(err)?,
            "catalog": self.runtime.catalog().snapshot()})).map_err(err)
    }
    fn process(&mut self, bytes: Vec<u8>, timestamp: u64, pose: RigidTransform) -> Result<String> {
        self.observe(bytes, timestamp, Some(pose))
    }
    fn observe(
        &mut self,
        bytes: Vec<u8>,
        timestamp: u64,
        pose: Option<RigidTransform>,
    ) -> Result<String> {
        if let Some(pose) = &pose
            && (pose.from_frame_id != self.cfg.optical_frame
                || pose.to_frame_id != self.cfg.map_frame)
        {
            return Err("camera pose frame mismatch".into());
        }
        if self.last_timestamp.is_some_and(|t| timestamp <= t) {
            return Err("capture time must advance on the configured clock".into());
        }
        if bytes.len() != self.cfg.width as usize * self.cfg.height as usize * 3 {
            return Err("RGB payload dimensions mismatch".into());
        }
        // Validate the labelled transform before publishing the camera observation.
        if let Some(pose) = &pose {
            QrAnchor {
                anchor_id: "validation".into(),
                payload: "validation".into(),
                side_length_m: 1.,
                pose_in_map: pose.clone(),
            }
            .validate_in_map(&self.map_definition())
            .map_err(err)?;
        }
        // One encoded observation serves both the local detector and remote viewers.
        // Encoding preserves image dimensions and the original capture timestamp.
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode(
                &bytes,
                self.cfg.width,
                self.cfg.height,
                image::ExtendedColorType::Rgb8,
            )
            .map_err(err)?;
        self.frames
            .publish(
                timestamp,
                Arc::new(VideoFrame {
                    width: self.cfg.width,
                    height: self.cfg.height,
                    encoding: "jpeg".into(),
                    bytes: jpeg.into(),
                }),
            )
            .map_err(err)?;
        self.last_timestamp = Some(timestamp);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut handled = std::collections::BTreeSet::new();
        let mut unresolved = Vec::new();
        loop {
            let expected = self
                .detector
                .detection_product()
                .latest_existing()
                .map_err(err)?
                .filter(|o| {
                    o.payload
                        .source_frame
                        .as_ref()
                        .is_some_and(|f| f.timestamp_ns == timestamp)
                })
                .map(|o| o.payload.codes.len());
            let entries = self.results.product().buffer().snapshot(0, u64::MAX);
            for entry in entries {
                let record = &entry.payload.payload;
                if record.source_frame.timestamp_ns != timestamp
                    || handled.contains(&record.detection_index)
                    || matches!(record.size, PortalSize::Pending)
                {
                    continue;
                }
                if record.capture_clock != self.cfg.capture_clock
                    || record.source_frame.camera_product != self.capture.product().reference()
                {
                    return Err("detection capture identity mismatch".into());
                }
                match &record.size {
                    PortalSize::Database { side_length_m, .. }
                    | PortalSize::LocalMaps { side_length_m, .. } => {
                        if let Some(pose) = &pose {
                            self.place(record, *side_length_m, pose)?;
                        }
                    }
                    _ => unresolved.push(record.detection.payload.clone()),
                }
                handled.insert(record.detection_index);
            }
            if expected.is_some_and(|n| handled.len() >= n) {
                break;
            }
            if let Some(error) = self.detector.last_error() {
                return Err(error);
            }
            if Instant::now() >= deadline {
                return Err("PortalDetector timed out; no latest-pose fallback".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut value: serde_json::Value = serde_json::from_str(&self.snapshot()?).map_err(err)?;
        value["unresolved_portals"] = serde_json::json!(unresolved);
        if pose.is_none() {
            let detection = self
                .detector
                .detection_product()
                .latest_existing()
                .map_err(err)?;
            if let Some(detection) = detection {
                value["detected_codes"] = serde_json::json!(detection.payload.codes.len());
                let mut seen_portals = Vec::new();
                for index in 0..detection.payload.codes.len() {
                    let payload = &detection.payload.codes[index].payload;
                    if let Ok(portal) = auki_qr_mapper::local::portal_id(payload) {
                        seen_portals.push(portal.as_str().to_owned());
                        // Legacy pose records identify a portal by shortcode, not
                        // the exact URL spelling printed on the physical marker.
                        let snapshot = self
                            .reference_map
                            .product()
                            .latest_existing()
                            .map_err(err)?
                            .ok_or("reference map missing")?;
                        let matches: Vec<_> = snapshot
                            .payload
                            .scenegraph
                            .anchors
                            .values()
                            .filter(|a| {
                                let Ok(id) = Uuid::parse_str(&a.anchor_id) else {
                                    return false;
                                };
                                let Ok(short) = auki_qr_mapper::local::portal_id(&a.payload) else {
                                    return false;
                                };
                                portal.matches(id, short.as_str())
                            })
                            .cloned()
                            .collect();
                        if matches.len() == 1 && matches[0].payload != *payload {
                            let mut anchor = matches[0].clone();
                            anchor.payload = payload.clone();
                            InMemoryTransport
                                .invoke(
                                    self.reference_map.upsert_qr(),
                                    self.maps.context().clone(),
                                    UpsertQr {
                                        expected_snapshot: self.reference_map.snapshot_reference(),
                                        anchor,
                                    },
                                )
                                .map_err(err)?;
                        }
                    }
                    let found = InMemoryTransport
                        .invoke(
                            self.reference_map.find_qr(),
                            self.maps.context().clone(),
                            FindQr {
                                payload: detection.payload.codes[index].payload.clone(),
                            },
                        )
                        .map_err(err)?
                        .result;
                    if found.anchors.len() != 1 {
                        continue;
                    }
                    let batch = InMemoryTransport
                        .invoke(
                            self.localizer.localize_once(),
                            self.maps.context().clone(),
                            LocalizeOnce {
                                detection_product: self.detector.detection_product().reference(),
                                detection_sequence: detection.sequence,
                                detection_index: index,
                                target_map: self.reference_map.snapshot_reference(),
                            },
                        )
                        .map_err(err)?
                        .result;
                    if let Some(estimate) = batch.estimates.first() {
                        value["localization"] = serde_json::to_value(&batch).map_err(err)?;
                        value["camera_to_map"] =
                            serde_json::to_value(&estimate.pose.camera_pose_in_map).map_err(err)?;
                        break;
                    }
                }
                value["seen_portal_ids"] = serde_json::json!(seen_portals);
            }
        }
        serde_json::to_string(&value).map_err(err)
    }
    fn map_definition(&self) -> MapDefinition {
        self.map
            .product()
            .latest_existing()
            .unwrap()
            .unwrap()
            .payload
            .scenegraph
            .map
            .clone()
    }
    fn place(
        &self,
        record: &PortalDetection,
        size: f64,
        camera_to_map: &RigidTransform,
    ) -> Result<()> {
        let corners = record
            .detection
            .refined_corners_px
            .as_ref()
            .unwrap_or(&record.detection.corners_px)
            .map(|p| Vector2::new(p.x, p.y));
        let solved = estimate_square_pose_from_pixels(
            corners,
            size,
            &Camera::pinhole(self.cfg.fx, self.cfg.fy, self.cfg.cx, self.cfg.cy).map_err(err)?,
        )
        .map_err(err)?;
        let optical = pose_tools::from_opengl_to_opencv(&solved.pose);
        let id = match &record.size {
            PortalSize::Database { portal_id, .. } => portal_id.to_string(),
            PortalSize::LocalMaps { candidates, .. } => candidates
                .iter()
                .next()
                .ok_or("no selected-map Portal")?
                .anchor
                .anchor_id
                .clone(),
            _ => return Err("unresolved size".into()),
        };
        let p = SpatialTransform {
            translation: Some(Vec3 {
                x: optical.position.x,
                y: optical.position.y,
                z: optical.position.z,
            }),
            orientation: Some(Quat {
                x: optical.rotation.x,
                y: optical.rotation.y,
                z: optical.rotation.z,
                w: optical.rotation.w,
            }),
        };
        let [x, y, z] = camera_to_map.translation;
        let [w, qx, qy, qz] = camera_to_map.rotation_wxyz;
        let c = SpatialTransform {
            translation: Some(Vec3 { x, y, z }),
            orientation: Some(Quat {
                x: qx,
                y: qy,
                z: qz,
                w,
            }),
        };
        let t = auki_geometry::compose_spatial_transforms(&p, &c).map_err(err)?;
        let v = t.translation.unwrap();
        let q = t.orientation.unwrap();
        let placement = RigidTransform {
            from_frame_id: format!("portal-{id}"),
            to_frame_id: self.cfg.map_frame.clone(),
            translation: [v.x, v.y, v.z],
            rotation_wxyz: [q.w, q.x, q.y, q.z],
        };
        let mapper = PortalMapper::new(self.maps.clone(), self.metadata.clone());
        let result = self
            .rt
            .block_on(mapper.lookup_helper(
                &record.detection.payload,
                self.cfg.domain_id,
                Some(AnchorPlacement {
                    map: self.map.clone(),
                    expected_snapshot: self.map.snapshot_reference(),
                    pose_in_map: placement.clone(),
                }),
                &CancellationToken::new(),
            ))
            .map_err(err)?;
        if matches!(result, EnsureQrResult::Known(_)) {
            InMemoryTransport
                .invoke(
                    self.map.upsert_qr(),
                    self.maps.context().clone(),
                    UpsertQr {
                        expected_snapshot: self.map.snapshot_reference(),
                        anchor: QrAnchor {
                            anchor_id: id,
                            payload: record.detection.payload.clone(),
                            side_length_m: size,
                            pose_in_map: placement,
                        },
                    },
                )
                .map_err(err)?;
        }
        Ok(())
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(endpoint) = self.endpoint.take() {
            let _ = self.rt.block_on(endpoint.close());
        }
        let _ = self.localizer.close(self.last_timestamp.unwrap_or(0));
        self.detector.close();
        self.reference_map.close();
        self.map.close();
    }
}

#[pyclass(name = "PortalMappingPipeline")]
struct Pipeline {
    engine: Mutex<Option<Engine>>,
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        let engine = self.engine.get_mut().expect("pipeline lock").take();
        if engine.is_some() {
            // Native workers may be finishing a Python metadata callback.
            Python::with_gil(|py| py.allow_threads(move || drop(engine)));
        }
    }
}
#[pymethods]
impl Pipeline {
    #[staticmethod]
    fn mount(
        py: Python<'_>,
        peer: &sdk_python::PyAukiPeer,
        config: &str,
        metadata: Py<PyAny>,
    ) -> PyResult<Self> {
        let cfg: Config =
            serde_json::from_str(config).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let protocols = peer.protocols();
        let id = protocols.peer_id().to_string();
        let engine = py
            .allow_threads(move || Engine::new(&id, protocols, cfg, Metadata::new(metadata)))
            .map_err(PyRuntimeError::new_err)?;
        Ok(Self {
            engine: Mutex::new(Some(engine)),
        })
    }
    fn process(
        &self,
        py: Python<'_>,
        rgb: Vec<u8>,
        timestamp_ns: u64,
        camera_to_map: &str,
    ) -> PyResult<String> {
        let pose = serde_json::from_str(camera_to_map)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        py.allow_threads(|| {
            self.engine
                .lock()
                .unwrap()
                .as_mut()
                .ok_or_else(|| "pipeline closed".to_string())?
                .process(rgb, timestamp_ns, pose)
        })
        .map_err(PyRuntimeError::new_err)
    }
    fn seed_reference(&self, py: Python<'_>, anchors: &str) -> PyResult<()> {
        let anchors: Vec<QrAnchor> =
            serde_json::from_str(anchors).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        py.allow_threads(|| -> Result<()> {
            let guard = self.engine.lock().unwrap();
            let e = guard.as_ref().ok_or("pipeline closed")?;
            for anchor in anchors {
                InMemoryTransport
                    .invoke(
                        e.reference_map.upsert_qr(),
                        e.maps.context().clone(),
                        UpsertQr {
                            expected_snapshot: e.reference_map.snapshot_reference(),
                            anchor,
                        },
                    )
                    .map_err(err)?;
            }
            Ok(())
        })
        .map_err(PyRuntimeError::new_err)
    }
    fn localize(&self, py: Python<'_>, rgb: Vec<u8>, timestamp_ns: u64) -> PyResult<String> {
        py.allow_threads(|| {
            self.engine
                .lock()
                .unwrap()
                .as_mut()
                .ok_or_else(|| "pipeline closed".to_string())?
                .observe(rgb, timestamp_ns, None)
        })
        .map_err(PyRuntimeError::new_err)
    }
    fn snapshot(&self, py: Python<'_>) -> PyResult<String> {
        py.allow_threads(|| {
            self.engine
                .lock()
                .unwrap()
                .as_ref()
                .ok_or_else(|| "pipeline closed".to_string())?
                .snapshot()
        })
        .map_err(PyRuntimeError::new_err)
    }
    fn close(&self, py: Python<'_>) {
        py.allow_threads(|| {
            self.engine.lock().unwrap().take();
        });
    }
}
#[pymodule]
fn auki_sdk(module: &Bound<'_, PyModule>) -> PyResult<()> {
    sdk_python::register_sdk(module)?;
    module.add_class::<Pipeline>()?;
    module.add_class::<odometry::OdometryPublisher>()?;
    Ok(())
}

#[cfg(test)]
mod tests;
