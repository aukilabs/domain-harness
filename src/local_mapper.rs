//! A new physical map frame anchored by the first valid robot pose. Home's
//! references are overlaid after alignment; they never become measurements.
use anyhow::{Result, anyhow, ensure};
use auki_components::*;
use auki_odometry::{Pose, PoseContract, PoseQuery, Tracking, pose_at};
use auki_registry::FrameRegistryEntry;
use auki_scenegraph::{
    MapDefinition, MapSnapshot, QrAnchor, RigidTransform,
    component::{MapComponent, MapComponentConfig, UpsertQr},
};
use auki_voxel_map::{
    DepthObservation, PortalVoxelMapper, SensorBinding, TimedSensorPose, VoxelMapComponent,
    VoxelMapDefinition, VoxelSnapshot,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

pub const FAMILY: &str = "galbot-local";
pub fn is_local_frame(id: &str) -> bool {
    id.strip_prefix("galbot-local:")
        .or_else(|| id.strip_prefix("home-live:"))
        .and_then(|s| s.strip_suffix(":zup").or_else(|| s.strip_suffix(":opengl")))
        .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
}
pub fn display_frame(id: &str) -> Result<String> {
    ensure!(is_local_frame(id), "not a local map frame");
    Ok(id
        .strip_suffix(":zup")
        .map(|s| format!("{s}:opengl"))
        .unwrap_or_else(|| id.into()))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraSample {
    pub timestamp_ns: u64,
    pub rgb_timestamp_ns: u64,
    pub transform_timestamp_ns: u64,
    pub width: u32,
    pub height: u32,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub gray_base64: String,
    pub calibration: crate::runner_voxels::SensorCalibration,
    pub hit_points_m: Vec<[f64; 3]>,
    pub hit_colors_srgb: Vec<[u8; 3]>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SensorEvent {
    Odometry {
        timestamp_ns: u64,
        pose: Pose,
    },
    Camera {
        sample: CameraSample,
    },
    Lidar {
        timestamp_ns: u64,
        reading: crate::runner_voxels::SensorReading,
        sensor_to_base: Pose,
    },
    Status {
        detail: String,
    },
}
impl ContractType for SensorEvent {
    const DATATYPE: &'static str = "spatial-harness.local-sensors/v1";
}

pub fn identity() -> Pose {
    Pose {
        translation: [0.; 3],
        rotation_xyzw: [0., 0., 0., 1.],
    }
}
fn spatial(p: &Pose) -> auki_datatypes::pose::SpatialTransform {
    let [x, y, z, w] = p.rotation_xyzw;
    auki_datatypes::pose::SpatialTransform {
        translation: Some(auki_datatypes::pose::Vec3 {
            x: p.translation[0],
            y: p.translation[1],
            z: p.translation[2],
        }),
        orientation: Some(auki_datatypes::pose::Quat { x, y, z, w }),
    }
}
fn pose(p: auki_datatypes::pose::SpatialTransform) -> Pose {
    let t = p.translation.unwrap();
    let q = p.orientation.unwrap();
    Pose {
        translation: [t.x, t.y, t.z],
        rotation_xyzw: [q.x, q.y, q.z, q.w],
    }
}
/// a(b(p)), using the SDK's explicitly ordered composition API.
pub fn compose(a: &Pose, b: &Pose) -> Result<Pose> {
    Ok(pose(auki_geometry::compose_spatial_transforms(
        &spatial(b),
        &spatial(a),
    )?))
}
pub fn inverse(p: &Pose) -> Result<Pose> {
    Ok(pose(auki_geometry::inverse_spatial_transform(&spatial(p))?))
}
pub(crate) fn rigid(p: &Pose, from: &str, to: &str) -> RigidTransform {
    let [x, y, z, w] = p.rotation_xyzw;
    RigidTransform {
        from_frame_id: from.into(),
        to_frame_id: to.into(),
        translation: p.translation,
        rotation_wxyz: [w, x, y, z],
    }
}
pub(crate) fn from_rigid(p: &RigidTransform) -> Pose {
    let [w, x, y, z] = p.rotation_wxyz;
    Pose {
        translation: p.translation,
        rotation_xyzw: [x, y, z, w],
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortalCalibration {
    pub portal_id: String,
    pub portal_pose_in_shared: Pose,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Origin {
    pub kind: String,
    pub capture_timestamp_ns: u64,
    pub capture_clock: ClockReference,
    pub odometry_to_local: Pose,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<PortalCalibration>,
}
/// Freeze the initial robot pose as the map origin. Portals never rebase it.
pub fn establish_origin(t: u64, clock: ClockReference, robot_to_odometry: &Pose) -> Result<Origin> {
    Ok(Origin {
        kind: "robot_boot_pose".into(),
        capture_timestamp_ns: t,
        capture_clock: clock,
        odometry_to_local: inverse(robot_to_odometry)?,
        calibration: None,
    })
}

/// T_shared_odometry = T_shared_portal * inverse(T_odometry_portal).
/// Different visible portals therefore yield the same canonical physical frame.
pub fn canonical_origin(
    t: u64,
    clock: ClockReference,
    marker_to_odometry: &Pose,
    reference: &QrAnchor,
) -> Result<Origin> {
    let target = from_rigid(&reference.pose_in_map);
    Ok(Origin {
        kind: "canonical_portals".into(),
        capture_timestamp_ns: t,
        capture_clock: clock,
        odometry_to_local: compose(&target, &inverse(marker_to_odometry)?)?,
        calibration: Some(PortalCalibration {
            portal_id: reference.anchor_id.clone(),
            portal_pose_in_shared: target,
        }),
    })
}
pub struct LocalMap {
    pub frame: String,
    pub revision: usize,
    pub home_portals: Option<crate::home_alignment::HomePortals>,
    pub origin: Option<Origin>,
    pub detail: String,
    owner: String,
    sensor_owner: String,
    camera_frame: String,
    remote_sources: Option<(ProductReference, ProductReference)>,
    portal_origin: bool,
    required_portal: Option<String>,
    canonical_reference: Option<MapSnapshot>,
    clock: ClockReference,
    contract: PoseContract,
    odometry: auki_odometry::OdometryComponent,
    history: BufferProductCapture<auki_odometry::PoseUpdate>,
    _source_component: Component,
    lidar_output: ConfiguredObservable<SensorEvent>,
    camera_source: ConfiguredObservable<SensorEvent>,
    lidar_capture: BufferProductCapture<SensorEvent>,
    camera_source_capture: BufferProductCapture<SensorEvent>,
    camera_component: Option<Component>,
    camera_output: Option<ConfiguredObservable<VideoFrame>>,
    camera_capture: Option<BufferProductCapture<VideoFrame>>,
    camera_calibration: Option<auki_qr_localizer::Calibration>,
    camera_key: Option<String>,
    pub ar_frame: Option<crate::ar_preview::Frame>,
    detector: auki_qr_detector::QrDetector,
    map: MapComponent,
    runtime: ComponentRuntime,
    pub voxels: Option<VoxelSnapshot>,
    recent_voxels: crate::recent_voxels::RecentVoxels,
    emit_scans: bool,
    localized_scans: Vec<crate::collaborative_mapper::LocalizedScan>,
    _voxel_component: Option<VoxelMapComponent>,
    sizes: crate::portal_sizes::PortalSizes,
    evidence: BTreeMap<String, Value>,
    pending: Vec<Observation<SensorEvent>>,
    pub latest_pose: Option<(u64, Pose)>,
    last_lidar_calibration: Option<Pose>,
}
impl LocalMap {
    pub fn new(runtime: &ComponentRuntime, owner: &str, session: &str) -> Result<Self> {
        let clock = crate::model::clock_ref(&crate::model::clock_entry(owner, session));
        let frame = format!("galbot-local:{session}:zup");
        let contract = PoseContract {
            from_frame: FrameRegistryEntry::ros_body(owner, "robot.base"),
            to_frame: FrameRegistryEntry::ros_body(owner, format!("robot.odometry.{session}")),
            clock: clock.clone(),
            session_id: session.into(),
        };
        let camera_frame = std::env::var("SPATIAL_MAPPING_CAMERA_FRAME")
            .unwrap_or_else(|_| crate::rgbd::FRAME.into());
        Self::with_contract(
            runtime,
            owner,
            session,
            frame,
            contract,
            None,
            false,
            None,
            FrameRegistryEntry::ros_optical(owner, camera_frame),
        )
    }
    /// Home computes its own map from original, authenticated sensor observations.
    pub fn from_runner(
        runtime: &ComponentRuntime,
        owner: &str,
        session: &str,
        contract: PoseContract,
        camera: ProductReference,
        lidar: ProductReference,
        required_portal: Option<String>,
    ) -> Result<Self> {
        let camera_frame =
            FrameRegistryEntry::ros_optical(&contract.clock.peer_id, crate::rgbd::FRAME);
        Self::from_runner_with_camera(
            runtime,
            owner,
            session,
            contract,
            camera,
            lidar,
            required_portal,
            camera_frame,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn from_runner_with_camera(
        runtime: &ComponentRuntime,
        owner: &str,
        session: &str,
        contract: PoseContract,
        camera: ProductReference,
        lidar: ProductReference,
        required_portal: Option<String>,
        camera_frame: FrameRegistryEntry,
    ) -> Result<Self> {
        contract.validate()?;
        ensure!(
            camera.peer_id == contract.clock.peer_id && lidar.peer_id == contract.clock.peer_id,
            "sensor source owner differs from capture clock"
        );
        Self::with_contract(
            runtime,
            owner,
            session,
            format!("home-live:{session}:zup"),
            contract,
            Some((camera, lidar)),
            true,
            required_portal,
            camera_frame,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn with_contract(
        runtime: &ComponentRuntime,
        owner: &str,
        session: &str,
        frame: String,
        contract: PoseContract,
        remote_sources: Option<(ProductReference, ProductReference)>,
        portal_origin: bool,
        required_portal: Option<String>,
        camera_frame: FrameRegistryEntry,
    ) -> Result<Self> {
        ensure!(
            crate::rgbd::supported_camera_frame(&camera_frame.frame_id)
                && camera_frame
                    == FrameRegistryEntry::ros_optical(
                        &contract.clock.peer_id,
                        &camera_frame.frame_id
                    ),
            "unsupported or misowned camera optical frame"
        );
        let clock = contract.clock.clone();
        let sensor_owner = contract.clock.peer_id.clone();
        let odometry =
            auki_odometry::OdometryComponent::new(runtime, "local-odometry", contract.clone())?;
        let history = auki_odometry::capture_pose_history(
            runtime,
            "local-odometry-history",
            &odometry,
            BufferLimits::entries(600),
        )?;
        let contract_for = |name: &str| ObservableContract {
            name: name.into(),
            datatype: SensorEvent::DATATYPE.into(),
            schema: SensorEvent::DATATYPE.into(),
            access: vec![ObservationAccess::FollowNew],
            exposure: Exposure::Cluster,
        };
        let source_component = runtime.component(
            ComponentSpec::new("local-sensor-source")
                .observable(contract_for("lidar"))
                .observable(contract_for("camera")),
        )?;
        let output_for = |name: &str, frame: FrameRegistryEntry| -> Result<_> {
            Ok(source_component.configured_observable(
                ConfiguredObservableSpec::new(
                    name,
                    format!("{name}-{session}"),
                    clock.clone(),
                    PayloadContract::Structured(StructuredPayloadContract {
                        modality: "sensor_observation".into(),
                        datatype: SensorEvent::DATATYPE.into(),
                        schema: SensorEvent::DATATYPE.into(),
                        observes: name.into(),
                        unit: Some("meters".into()),
                    }),
                )
                .in_registered_frame(frame),
            )?)
        };
        let lidar_output = output_for(
            "lidar",
            FrameRegistryEntry::ros_body(owner, "lidar_base_link"),
        )?;
        let camera_source = output_for("camera", camera_frame.clone())?;
        source_component.expose()?;
        let size = |s: &SensorEvent| serde_json::to_vec(s).map_or(usize::MAX, |s| s.len());
        let lidar_capture = runtime.capture_buffer(
            "local-lidar-history",
            &lidar_output,
            BufferLimits::entries(3),
            size,
        )?;
        let camera_source_capture = runtime.capture_buffer(
            "local-camera-observations",
            &camera_source,
            BufferLimits::entries(3),
            size,
        )?;
        let writer = owner.to_owned();
        let map = MapComponent::new(
            runtime,
            MapComponentConfig {
                component_id: "observed-portals".into(),
                publication_id: session.into(),
                clock: clock.clone(),
                map: MapDefinition {
                    map_id: format!("observed-{session}"),
                    name: Some(
                        if portal_origin {
                            "Home observed portal map"
                        } else {
                            "Galbot observed local map"
                        }
                        .into(),
                    ),
                    domain_reference: None,
                    frame: crate::conventions::frame(&frame)?,
                },
            },
            now,
            |_| true,
            move |caller| {
                caller.caller_peer_id == writer && caller.caller_component_id == "local-mapper"
            },
        )?;
        Ok(Self {
            frame,
            revision: 0,
            home_portals: None,
            origin: None,
            detail: if portal_origin {
                "Waiting for UCOF1J40N7O in raw camera observations"
            } else {
                "Waiting for first valid robot pose"
            }
            .into(),
            owner: owner.into(),
            sensor_owner,
            camera_frame: camera_frame.frame_id,
            remote_sources,
            portal_origin,
            required_portal,
            canonical_reference: None,
            clock,
            contract,
            odometry,
            history,
            _source_component: source_component,
            lidar_output,
            camera_source,
            lidar_capture,
            camera_source_capture,
            camera_component: None,
            camera_output: None,
            camera_capture: None,
            camera_calibration: None,
            camera_key: None,
            ar_frame: None,
            detector: auki_qr_detector::QrDetector::new(Default::default()),
            map,
            runtime: runtime.clone(),
            voxels: None,
            recent_voxels: Default::default(),
            emit_scans: false,
            localized_scans: Vec::new(),
            _voxel_component: None,
            sizes: serde_json::from_str(include_str!("../config/portals.json"))?,
            evidence: BTreeMap::new(),
            pending: vec![],
            latest_pose: None,
            last_lidar_calibration: None,
        })
    }
    /// Home uses only the localization stage; one shared writer integrates rays.
    pub fn emit_localized_scans(&mut self) {
        self.emit_scans = true;
    }
    pub fn calibrate_against(&mut self, canonical: &MapSnapshot) -> Result<()> {
        ensure!(
            self.origin.is_none(),
            "cannot replace alignment after mapping starts"
        );
        self.canonical_reference = Some(crate::collaborative_mapper::reference_in_frame(
            canonical,
            &self.frame,
        )?);
        self.required_portal = None;
        self.detail = "Waiting for any portal in Home's canonical record".into();
        Ok(())
    }
    pub fn take_localized_scans(&mut self) -> Vec<crate::collaborative_mapper::LocalizedScan> {
        std::mem::take(&mut self.localized_scans)
    }
    pub fn export_sources(
        &self,
        endpoint: &auki_component_protocol::ComponentProtocolEndpoint,
    ) -> Result<()> {
        endpoint.export_product(&self.history.product())?;
        endpoint.export_product(&self.lidar_capture.product())?;
        endpoint.export_product(&self.camera_source_capture.product())?;
        endpoint.export_product(&self.map.product())?;
        Ok(())
    }
    pub fn export_camera_preview(
        &self,
        endpoint: &auki_component_protocol::ComponentProtocolEndpoint,
    ) -> Result<bool> {
        if let Some(capture) = &self.camera_capture {
            endpoint.export_product(&capture.product())?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub fn portals(&self) -> Result<MapSnapshot> {
        Ok((*self
            .map
            .product()
            .latest_existing()?
            .ok_or_else(|| anyhow!("map snapshot missing"))?
            .payload)
            .clone())
    }
    pub fn accept_home(&mut self, scene: crate::network::CanonicalScene) -> Result<()> {
        let observed = self.portals()?;
        let anchor = self
            .latest_shared_portal(&scene.portals)
            .ok_or_else(|| anyhow!("No observed portal is known to Home"))?;
        let home = crate::home_alignment::HomePortals {
            portals: scene.portals,
            peer_id: scene.authority_peer_id,
            revision: scene.revision,
            anchor,
        };
        home.align(&observed)?;
        self.home_portals = Some(home);
        Ok(())
    }
    fn latest_shared_portal(&self, reference: &MapSnapshot) -> Option<String> {
        self.evidence
            .iter()
            .filter(|(id, _)| reference.scenegraph.anchors.contains_key(*id))
            .filter_map(|(id, evidence)| evidence["capture_timestamp_ns"].as_u64().map(|t| (t, id)))
            .max()
            .map(|(_, id)| id.clone())
    }
    pub fn metadata(&self) -> Value {
        json!({"local_mapping":{"status":if self.origin.is_some(){"mapping"}else{"waiting_for_odometry"},"detail":self.detail,"origin":self.origin,
            "voxel_policy":{"kind":"latest_observations_per_cell","window":3,"free_space":"SDK measured rays only","unseen_cells":"retained"},"alignment_to_home":null,"portal_poses":"observed","portal_color":"blue","portal_axes":"+X printed right; +Y printed top; +Z out of marker",
            "observed_portals":self.evidence,"robot_pose":self.latest_pose.as_ref().map(|(t,p)|json!({"capture_timestamp_ns":t,"clock":self.clock,"pose":p,"frame":self.frame}))},
            "frames":[self.contract.from_frame,self.contract.to_frame],"clocks":[crate::model::clock_entry(&self.sensor_owner,&self.contract.session_id)]})
    }
    pub fn accept(&mut self, event: SensorEvent) -> Result<()> {
        match event {
            SensorEvent::Odometry { timestamp_ns, pose } => {
                pose.validate()?;
                self.odometry
                    .publish(timestamp_ns, Tracking::Valid { pose: pose.clone() })?;
                if self.origin.is_none() && !self.portal_origin {
                    self.origin = Some(establish_origin(timestamp_ns, self.clock.clone(), &pose)?);
                    self.initialize_voxels()?;
                    self.detail = "Robot boot origin · mapping measured geometry".into();
                    self.revision += 1;
                }
                if let Some(origin) = &self.origin {
                    self.latest_pose =
                        Some((timestamp_ns, compose(&origin.odometry_to_local, &pose)?));
                }
            }
            SensorEvent::Status { detail } => self.detail = detail,
            event => {
                let t = match &event {
                    SensorEvent::Camera { sample } => sample.timestamp_ns,
                    SensorEvent::Lidar { timestamp_ns, .. } => *timestamp_ns,
                    _ => unreachable!(),
                };
                let output = if matches!(&event, SensorEvent::Camera { .. }) {
                    &self.camera_source
                } else {
                    &self.lidar_output
                };
                let (o, _) = output.publish(t, Arc::new(event))?;
                ensure!(
                    self.lidar_capture.errors().is_empty()
                        && self.camera_source_capture.errors().is_empty(),
                    "sensor retention failed"
                );
                if self.pending.len() >= 8 {
                    self.pending.remove(0);
                }
                self.pending.push(o);
            }
        }
        self.drain()
    }
    pub fn accept_remote(&mut self, observation: Observation<SensorEvent>) -> Result<()> {
        let t = match &*observation.payload {
            SensorEvent::Camera { sample } => sample.timestamp_ns,
            SensorEvent::Lidar { timestamp_ns, .. } => *timestamp_ns,
            _ => anyhow::bail!("unexpected remote sensor event"),
        };
        ensure!(
            t == observation.timestamp_ns,
            "sensor envelope timestamp mismatch"
        );
        if self.pending.len() >= 8 {
            self.pending.remove(0);
        }
        self.pending.push(observation);
        self.drain()
    }
    pub fn accept_pose(
        &mut self,
        timestamp_ns: u64,
        update: &auki_odometry::PoseUpdate,
    ) -> Result<()> {
        ensure!(
            update.contract == self.contract,
            "odometry session/frame changed"
        );
        match &update.tracking {
            Tracking::Valid { pose } => self.accept(SensorEvent::Odometry {
                timestamp_ns,
                pose: pose.clone(),
            }),
            Tracking::Lost => {
                self.odometry.publish(timestamp_ns, Tracking::Lost)?;
                self.latest_pose = None;
                Ok(())
            }
        }
    }
    fn camera_reference(&self) -> ProductReference {
        self.remote_sources
            .as_ref()
            .map(|s| s.0.clone())
            .unwrap_or_else(|| self.camera_source_capture.product().reference())
    }
    fn lidar_reference(&self) -> ProductReference {
        self.remote_sources
            .as_ref()
            .map(|s| s.1.clone())
            .unwrap_or_else(|| self.lidar_capture.product().reference())
    }
    pub fn ar_pose_history(&self) -> crate::ar_preview::PoseHistory {
        crate::ar_preview::PoseHistory {
            product: self.history.product(),
            contract: self.contract.clone(),
        }
    }
    fn at(&self, t: u64) -> Result<Option<Pose>> {
        match pose_at(
            &self.history.product(),
            &PoseQuery {
                contract: self.contract.clone(),
                timestamp_ns: t,
                max_gap_ns: 200_000_000,
            },
        ) {
            Ok(p) => Ok(Some(p.pose)),
            Err(
                auki_odometry::PoseError::OutsideHistory
                | auki_odometry::PoseError::GapTooLarge
                | auki_odometry::PoseError::TrackingLost,
            ) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn drain(&mut self) -> Result<()> {
        let pending = std::mem::take(&mut self.pending);
        for o in pending {
            let t = match &*o.payload {
                SensorEvent::Camera { sample } => sample.timestamp_ns,
                SensorEvent::Lidar { timestamp_ns, .. } => *timestamp_ns,
                _ => continue,
            };
            let Some(base) = self.at(t)? else {
                if self
                    .history
                    .product()
                    .latest_existing()?
                    .is_none_or(|last| last.timestamp_ns < t + 2_000_000_000)
                {
                    self.pending.push(o);
                }
                continue;
            };
            match &*o.payload {
                SensorEvent::Camera { sample } => self.camera(sample, &base, o.sequence)?,
                SensorEvent::Lidar {
                    reading,
                    sensor_to_base,
                    ..
                } => {
                    if self.origin.is_none() {
                        continue;
                    }
                    // Keep the existing stationary-base LiDAR admission gate.
                    let (Some(a), Some(b)) = (
                        self.at(t.saturating_sub(150_000_000))?,
                        self.at(t + 150_000_000)?,
                    ) else {
                        self.pending.push(o);
                        continue;
                    };
                    let distance = a
                        .translation
                        .iter()
                        .zip(b.translation)
                        .map(|(a, b)| (a - b).powi(2))
                        .sum::<f64>()
                        .sqrt();
                    let dot = a
                        .rotation_xyzw
                        .iter()
                        .zip(b.rotation_xyzw)
                        .map(|(a, b)| a * b)
                        .sum::<f64>()
                        .abs()
                        .min(1.);
                    if distance >= 0.01 || 2. * dot.acos() >= 0.0174533 {
                        continue;
                    }
                    sensor_to_base.validate()?;
                    if let Some(old) = &self.last_lidar_calibration {
                        ensure!(
                            old.translation
                                .iter()
                                .zip(sensor_to_base.translation)
                                .all(|(a, b)| (a - b).abs() < 1e-8)
                                && old
                                    .rotation_xyzw
                                    .iter()
                                    .zip(sensor_to_base.rotation_xyzw)
                                    .all(|(a, b)| (a - b).abs() < 1e-8),
                            "LiDAR mounting transform changed"
                        );
                    }
                    self.last_lidar_calibration = Some(sensor_to_base.clone());
                    let cloud = crate::runner_voxels::decode_colored(reading, t)?;
                    self.integrate(
                        t,
                        o.sequence,
                        &base,
                        sensor_to_base,
                        FrameRegistryEntry::ros_body(&self.sensor_owner, "lidar_base_link"),
                        cloud.points,
                        cloud.colors,
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn camera(&mut self, s: &CameraSample, base: &Pose, sequence: u64) -> Result<()> {
        ensure!(
            s.timestamp_ns == s.rgb_timestamp_ns && s.timestamp_ns == s.transform_timestamp_ns,
            "camera timestamp mismatch"
        );
        // A delayed image cannot roll canonical localization back across a
        // later portal correction. The physical map frame itself stays fixed.
        if self.canonical_reference.is_some()
            && self
                .origin
                .as_ref()
                .is_some_and(|o| s.timestamp_ns < o.capture_timestamp_ns)
        {
            return Ok(());
        }
        s.calibration.validate()?;
        ensure!(
            s.calibration.sensor_frame == self.camera_frame
                && (64..=4096).contains(&s.width)
                && (64..=4096).contains(&s.height),
            "unexpected camera frame/shape"
        );
        let key = serde_json::to_string(&(
            s.width,
            s.height,
            s.fx,
            s.fy,
            s.cx,
            s.cy,
            &s.calibration.calibration_source,
        ))?;
        if let Some(previous) = &self.camera_key {
            ensure!(
                previous == &key,
                "camera intrinsics changed; new local session required"
            );
        }
        let bytes = STANDARD.decode(&s.gray_base64)?;
        ensure!(
            bytes.len() == s.width as usize * s.height as usize,
            "camera buffer size mismatch"
        );
        if self.camera_output.is_none() {
            let c = self
                .runtime
                .component(
                    ComponentSpec::new("local-camera").observable(ObservableContract {
                        name: "frames".into(),
                        datatype: VideoFrame::DATATYPE.into(),
                        schema: "auki.video-frame/v1".into(),
                        access: vec![ObservationAccess::FollowNew],
                        exposure: Exposure::Cluster,
                    }),
                )?;
            let output = c.configured_observable(
                ConfiguredObservableSpec::new(
                    "frames",
                    format!("camera-{}", self.contract.session_id),
                    self.clock.clone(),
                    PayloadContract::Camera(CameraPayloadContract {
                        datatype: VideoFrame::DATATYPE.into(),
                        schema: "auki.video-frame/v1".into(),
                        encoding: "luma8".into(),
                        width: s.width,
                        height: s.height,
                        nominal_frame_rate_hz: Some(1),
                        observes: format!("rectified {}", self.camera_frame),
                    }),
                )
                .in_registered_frame(FrameRegistryEntry::ros_optical(
                    &self.owner,
                    &self.camera_frame,
                )),
            )?;
            c.expose()?;
            let capture = self.runtime.capture_buffer(
                "local-camera-history",
                &output,
                BufferLimits::entries(2),
                |f: &VideoFrame| f.bytes.len(),
            )?;
            self.camera_calibration = Some(auki_qr_localizer::Calibration::for_product(
                "wrist-rectified",
                &capture.product(),
                auki_qr_localizer::Intrinsics {
                    fx: s.fx,
                    fy: s.fy,
                    cx: s.cx,
                    cy: s.cy,
                    distortion: auki_qr_localizer::Distortion::None,
                },
            )?);
            self.camera_component = Some(c);
            self.camera_output = Some(output);
            self.camera_capture = Some(capture);
            self.camera_key = Some(key);
        }
        self.camera_output.as_ref().unwrap().publish(
            s.timestamp_ns,
            Arc::new(VideoFrame {
                width: s.width,
                height: s.height,
                encoding: "luma8".into(),
                bytes: bytes.clone().into(),
            }),
        )?;
        let detections = self.detector.detect_luma(
            &bytes,
            s.width as usize,
            s.height as usize,
            s.width as usize,
        )?;
        let camera_odom = compose(base, &s.calibration.sensor_to_base)?;
        let mut accepted = 0;
        for code in detections.codes {
            if code.mirrored {
                continue;
            }
            let Some(short) = portal_code(&code.payload) else {
                continue;
            };
            let Some(size) = self.sizes.encoded_size_m(short)? else {
                continue;
            };
            let snapshot = self.portals()?;
            let template = QrAnchor {
                anchor_id: short.into(),
                payload: code.payload.clone(),
                side_length_m: size,
                pose_in_map: rigid(&identity(), &format!("portal-{short}"), &self.frame),
            };
            let corners = code
                .refined_corners_px
                .as_ref()
                .unwrap_or(&code.corners_px)
                .map(|p| [p.x, p.y]);
            let estimate = match auki_qr_localizer::estimate_camera_pose(
                &snapshot.scenegraph.map,
                &template,
                corners,
                self.camera_calibration.as_ref().unwrap(),
                Default::default(),
            ) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let marker_camera = inverse(&from_rigid(&estimate.camera_pose_in_map))?;
            if let Some(reference) = &self.canonical_reference
                && let Some(anchor) = reference.scenegraph.anchors.get(short)
                && self
                    .origin
                    .as_ref()
                    .is_none_or(|o| s.timestamp_ns > o.capture_timestamp_ns)
            {
                ensure!(
                    (anchor.side_length_m - size).abs() < 1e-8,
                    "canonical portal size differs from observation"
                );
                // Reacquire through any known portal, including after a manual
                // push that wheel odometry did not measure. Correct future
                // poses/observations; never rebase existing map geometry.
                self.origin = Some(canonical_origin(
                    s.timestamp_ns,
                    self.clock.clone(),
                    &compose(&camera_odom, &marker_camera)?,
                    anchor,
                )?);
            }
            if self.origin.is_none() && self.portal_origin {
                if self
                    .required_portal
                    .as_ref()
                    .is_some_and(|required| required != short)
                {
                    continue;
                }
                let measured_marker = compose(&camera_odom, &marker_camera)?;
                self.origin = Some(if let Some(reference) = &self.canonical_reference {
                    let Some(anchor) = reference.scenegraph.anchors.get(short) else {
                        continue;
                    };
                    ensure!(
                        (anchor.side_length_m - size).abs() < 1e-8,
                        "canonical portal size differs from observation"
                    );
                    canonical_origin(s.timestamp_ns, self.clock.clone(), &measured_marker, anchor)?
                } else {
                    Origin {
                        kind: format!("portal:{short}"),
                        capture_timestamp_ns: s.timestamp_ns,
                        capture_clock: self.clock.clone(),
                        odometry_to_local: inverse(&measured_marker)?,
                        calibration: None,
                    }
                });
            }
            let origin = self
                .origin
                .as_ref()
                .ok_or_else(|| anyhow!("Waiting for robot odometry"))?;
            let measured = compose(
                &origin.odometry_to_local,
                &compose(&camera_odom, &marker_camera)?,
            )?;
            let placement = measured.clone();
            let anchor = QrAnchor {
                pose_in_map: rigid(&placement, &format!("portal-{short}"), &self.frame),
                ..template
            };
            InMemoryTransport
                .invoke(
                    self.map.upsert_qr(),
                    InvocationContext {
                        invocation_id: uuid::Uuid::new_v4().to_string(),
                        caller_peer_id: self.owner.clone(),
                        caller_component_id: "local-mapper".into(),
                    },
                    UpsertQr {
                        expected_snapshot: self.map.snapshot_reference(),
                        anchor,
                    },
                )
                .map_err(|e| anyhow!("{e:?}"))?;
            let n = self
                .evidence
                .get(short)
                .and_then(|v| v["observations"].as_u64())
                .unwrap_or(0)
                + 1;
            self.evidence.insert(short.into(),json!({"observations":n,"capture_timestamp_ns":s.timestamp_ns,"clock":self.clock,"camera_product":self.camera_reference(),"reprojection_rms_px":estimate.reprojection_rms_px,"latest_measured_pose":measured}));
            accepted += 1;
            self.revision += 1;
        }
        if let Some(home) = &self.home_portals
            && let Some(anchor) = self.latest_shared_portal(&home.portals)
        {
            self.home_portals.as_mut().unwrap().anchor = anchor;
        }
        self.detail = if let Some(origin) = &self.origin {
            format!(
                "{} · {accepted} portals observed in latest image",
                origin.kind
            )
        } else {
            "Waiting for a known-size portal to establish Home map origin".into()
        };
        if self.origin.is_some() {
            self.initialize_voxels()?;
            self.latest_pose = Some((
                s.timestamp_ns,
                compose(&self.origin.as_ref().unwrap().odometry_to_local, base)?,
            ));
            if self.emit_scans
                && self
                    .origin
                    .as_ref()
                    .is_some_and(|o| o.kind == "canonical_portals")
            {
                self.ar_frame = Some(crate::ar_preview::Frame {
                    source: self.camera_reference(),
                    clock: self.clock.clone(),
                    timestamp_ns: s.timestamp_ns,
                    sequence,
                    width: s.width,
                    height: s.height,
                    intrinsics: [s.fx, s.fy, s.cx, s.cy],
                    gray_base64: s.gray_base64.clone(),
                    camera_to_map: compose(
                        &self.origin.as_ref().unwrap().odometry_to_local,
                        &camera_odom,
                    )?,
                    origin: self.origin.clone().unwrap(),
                    received: std::time::Instant::now(),
                });
            }
            if !s.hit_points_m.is_empty() {
                let sample = crate::rgbd::Capture {
                    timestamp_ns: s.timestamp_ns,
                    rgb_timestamp_ns: s.rgb_timestamp_ns,
                    transform_timestamp_ns: s.transform_timestamp_ns,
                    hit_points_m: s.hit_points_m.clone(),
                    hit_colors_srgb: s.hit_colors_srgb.clone(),
                    calibration: s.calibration.clone(),
                };
                sample.validate_in_frame(&self.camera_frame)?;
                self.integrate(
                    s.timestamp_ns,
                    sequence,
                    base,
                    &s.calibration.sensor_to_base,
                    FrameRegistryEntry::ros_optical(&self.sensor_owner, &self.camera_frame),
                    s.hit_points_m.clone(),
                    s.hit_colors_srgb.iter().copied().map(Some).collect(),
                )?;
            }
        }
        Ok(())
    }
    fn initialize_voxels(&mut self) -> Result<()> {
        if self.voxels.is_none() {
            let portals = self.portals()?;
            let source = self.camera_reference();
            let frame = FrameRegistryEntry::ros_optical(&self.sensor_owner, &self.camera_frame);
            let mut definition = VoxelMapDefinition::from_scene_map(
                &self.owner,
                "local-voxels",
                self.map.snapshot_reference(),
                &portals,
                crate::conventions::registry(&self.owner, &self.frame)?,
                source,
                frame,
                self.clock.clone(),
                0.1,
            )?;
            crate::voxel_color::enable(&mut definition);
            let component = VoxelMapComponent::new(
                &self.runtime,
                "local-voxel-mapper",
                &format!("local-voxels-{}", self.contract.session_id),
                self.clock.clone(),
                definition,
                now,
            )?;
            self.voxels = Some((*component.product().latest_existing()?.unwrap().payload).clone());
            self._voxel_component = Some(component);
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn integrate(
        &mut self,
        t: u64,
        sequence: u64,
        base: &Pose,
        sensor_base: &Pose,
        sensor_frame: FrameRegistryEntry,
        points: Vec<[f64; 3]>,
        colors: Vec<Option<[u8; 3]>>,
    ) -> Result<()> {
        let origin = self
            .origin
            .as_ref()
            .ok_or_else(|| anyhow!("no robot boot origin"))?;
        if t < origin.capture_timestamp_ns {
            return Ok(());
        }
        let sensor_local = compose(&origin.odometry_to_local, &compose(base, sensor_base)?)?;
        let source = if sensor_frame.frame_id == self.camera_frame {
            self.camera_reference()
        } else {
            self.lidar_reference()
        };
        let binding = SensorBinding {
            sensor_product: source.clone(),
            sensor_frame: sensor_frame.clone(),
            observation_clock: self.clock.clone(),
        };
        self.initialize_voxels()?;
        let mut saved = self.voxels.as_ref().unwrap().clone();
        if !saved.definition.sources.contains(&binding) {
            saved.definition.sources.push(binding.clone());
        }
        let observation = DepthObservation {
            source,
            sequence,
            timestamp_ns: t,
            clock: self.clock.clone(),
            sensor_frame_id: sensor_frame.frame_id.clone(),
            hit_points_m: points,
            pose: TimedSensorPose {
                timestamp_ns: t,
                clock: self.clock.clone(),
                sensor_to_map: rigid(&sensor_local, &sensor_frame.frame_id, &self.frame),
                portal_snapshot: saved.definition.portal_snapshot.clone(),
            },
        };
        if self.emit_scans {
            ensure!(
                self.localized_scans.len() < 32,
                "localized scan consumer fell behind"
            );
            self.localized_scans
                .push(crate::collaborative_mapper::LocalizedScan {
                    observation,
                    binding,
                    colors,
                    origin: self.origin.clone().unwrap(),
                });
            self.revision += 1;
            return Ok(());
        }
        let mut update =
            PortalVoxelMapper::new(saved.definition.clone())?.map_observation(&observation)?;
        crate::voxel_color::add(
            &mut update,
            &observation.hit_points_m,
            &colors,
            &spatial(&sensor_local),
            0.1,
        )?;
        // Multiple rays in one capture count once; an endpoint wins over a
        // crossing ray. Keep the last three captures that actually saw a cell.
        let endpoints = auki_mappers::Voxelizer::new(0.1, 16)?.map_sensor_points(
            observation
                .hit_points_m
                .iter()
                .map(|p| auki_datatypes::pose::Vec3 {
                    x: p[0],
                    y: p[1],
                    z: p[2],
                }),
            &spatial(&sensor_local),
            1.,
        )?;
        let mut recent = self.recent_voxels.clone();
        let checkpoint = recent.observe(&update, &endpoints)?;
        let mut accumulator = saved.accumulator()?;
        accumulator.apply(&checkpoint)?;
        saved.checkpoint = accumulator.checkpoint_update().encode_to_vec();
        saved.integrated_observations += 1;
        saved.latest_observation = Some((&observation).into());
        saved.accumulator()?;
        self.voxels = Some(saved);
        self.recent_voxels = recent;
        self.revision += 1;
        Ok(())
    }
}
pub fn portal_code(payload: &str) -> Option<&str> {
    let (prefix, code) = payload.rsplit_once('/')?;
    (prefix.eq_ignore_ascii_case("https://r8.hr")
        && code.len() == 11
        && code
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
    .then_some(code)
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}
