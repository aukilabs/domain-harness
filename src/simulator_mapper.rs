//! Robot-owned mapping of measured simulated LiDAR rays, never world geometry.
use crate::{
    network::{CanonicalScene, SCENE_SCHEMA},
    simulated_lidar::Scan,
};
use anyhow::{Result, ensure};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;
use auki_scenegraph::{
    MapDefinition,
    component::{MapComponent, MapComponentConfig},
};
use auki_voxel_map::{
    DepthObservation, PortalVoxelMapper, TimedSensorPose, VoxelMapDefinition, VoxelSnapshot,
};
use prost::Message;
use std::sync::{Arc, Mutex};

pub const PRODUCT: &str = "simulator-local-scene";
type Inbox = Arc<Mutex<Option<Observation<Scan>>>>;
pub struct Mapper {
    _component: Component,
    _input: InputPort<Observation<Scan>>,
    _binding: crate::simulated_lidar::LocalScanInput,
    inbox: Inbox,
    output: ConfiguredObservable<CanonicalScene>,
    capture: BufferProductCapture<CanonicalScene>,
    _map: MapComponent,
    scene: CanonicalScene,
    recent: crate::recent_voxels::RecentVoxels,
    origin: Option<auki_odometry::Pose>,
    source: RetainedProduct<Scan>,
    last: Option<u64>,
    session_clock: auki_time::SessionClock,
}
impl Mapper {
    /// The simulated flat floor is world Y=0; express it in the boot frame.
    /// Robot/sensor orientation is yaw-only, so this is a height translation.
    pub fn floor_m(&self) -> Option<f64> { self.origin.as_ref().map(|p| p.translation[1]) }
    pub fn mount(
        runtime: &ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        lidar: &crate::simulated_lidar::Lidar,
        session_clock: auki_time::SessionClock,
    ) -> Result<Self> {
        let source = lidar.product();
        let owner = runtime.peer_id();
        let session = uuid::Uuid::new_v4().to_string();
        let frame = format!("simulator-local:{session}:opengl");
        let clock = session_clock.registry_entry();
        let reference = crate::model::clock_ref(&clock);
        ensure!(clock.peer_id == owner && source.producer.clock == reference,
            "Simulator mapper and LiDAR must use the same peer session clock");
        let map = MapComponent::new(
            runtime,
            MapComponentConfig {
                component_id: "simulator-local-frame".into(),
                publication_id: session.clone(),
                clock: reference.clone(),
                map: MapDefinition {
                    map_id: format!("simulator-{session}"),
                    name: Some("Robot-local LiDAR map".into()),
                    domain_reference: None,
                    frame: crate::conventions::frame(&frame)?,
                },
            },
            { let clock = session_clock.clone(); move || clock.now_ns() },
            |_| true,
            |_| false,
        )?;
        let portals = (*map.product().latest_existing()?.unwrap().payload).clone();
        let sensor_frame = source
            .producer
            .spatial_frame
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("LiDAR frame missing"))?
            .definition
            .clone();
        let definition = VoxelMapDefinition::from_scene_map(
            owner,
            "simulator-voxels",
            map.snapshot_reference(),
            &portals,
            crate::conventions::registry(owner, &frame)?,
            source.reference(),
            sensor_frame,
            source.producer.clock.clone(),
            0.1,
        )?;
        // Empty SDK checkpoint; no geometry is supplied by the world asset.
        let checkpoint = auki_datatypes::map::MapUpdate {
            voxel_chunks: vec![],
            checkpoint: Some(Default::default()),
        }
        .encode_to_vec();
        let scene = CanonicalScene {
            schema: SCENE_SCHEMA.into(),
            sdk_revision: crate::model::SDK_REV.into(),
            authority_peer_id: owner.into(),
            frame: frame.clone(),
            publication_clock: clock,
            portals,
            voxels: VoxelSnapshot {
                definition,
                checkpoint,
                integrated_observations: 0,
                latest_observation: None,
            },
            objects: vec![],
            registries: serde_json::json!({"local_mapping":{"status":"waiting_for_lidar","alignment_to_home":null}}),
        };
        scene.validate(owner)?;
        let component = runtime.component(
            ComponentSpec::new("simulator-local-mapper")
                .product_input(ProductInputContract {
                    name: "lidar".into(),
                    form: ProductForm::Buffer,
                    datatype: Scan::DATATYPE.into(),
                    schema: Scan::DATATYPE.into(),
                    exposure: Exposure::Cluster,
                })
                .observable(ObservableContract {
                    name: "scene".into(),
                    datatype: SCENE_SCHEMA.into(),
                    schema: SCENE_SCHEMA.into(),
                    access: vec![ObservationAccess::FollowNew],
                    exposure: Exposure::Cluster,
                }),
        )?;
        let inbox: Inbox = Arc::new(Mutex::new(None));
        let received = inbox.clone();
        let input = InputPort::<Observation<Scan>>::new("simulator.mapper.lidar", move |entry| {
            *received.lock().unwrap() = Some(entry.payload.clone());
        });
        let binding = lidar.bind_input(&component, &input)?;
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "scene",
                session,
                reference,
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "scenegraph".into(),
                    datatype: SCENE_SCHEMA.into(),
                    schema: SCENE_SCHEMA.into(),
                    observes: "robot_local_voxels".into(),
                    unit: Some("meters".into()),
                }),
            )
            .in_registered_frame(crate::conventions::registry(owner, &frame)?),
        )?;
        component.expose()?;
        let capture = runtime.capture_buffer(
            PRODUCT,
            &output,
            BufferLimits {
                max_entries: Some(1),
                max_bytes: Some(crate::network::MAX_SCENE_BYTES),
                target_duration: None,
            },
            |s: &CanonicalScene| serde_json::to_vec(s).map_or(usize::MAX, |b| b.len()),
        )?;
        endpoint.export_product(&capture.product())?;
        Ok(Self {
            _component: component,
            _input: input,
            _binding: binding,
            inbox,
            output,
            capture,
            _map: map,
            scene,
            recent: Default::default(),
            origin: None,
            source,
            last: None,
            session_clock,
        })
    }
    pub fn product(&self) -> RetainedProduct<CanonicalScene> {
        self.capture.product()
    }
    pub fn tick(&mut self) -> Result<bool> {
        let Some(o) = self.inbox.lock().unwrap().take() else {
            return Ok(false);
        };
        if self.last.is_some_and(|last| o.timestamp_ns <= last) {
            return Ok(false);
        }
        self.integrate(&o.payload, o.sequence)?;
        self.last = Some(o.timestamp_ns);
        Ok(true)
    }
    fn integrate(&mut self, scan: &Scan, sequence: u64) -> Result<()> {
        scan.validate()?;
        let mut world_sensor = auki_odometry::Pose {
            translation: scan.world_position.map(f64::from),
            rotation_xyzw: scan.world_rotation_xyzw.map(f64::from),
        };
        let norm = world_sensor
            .rotation_xyzw
            .iter()
            .map(|v| v * v)
            .sum::<f64>()
            .sqrt();
        world_sensor
            .rotation_xyzw
            .iter_mut()
            .for_each(|v| *v /= norm);
        let mount = auki_odometry::Pose {
            translation: [0., 0.65, -0.28],
            ..crate::local_mapper::identity()
        };
        let world_body =
            crate::local_mapper::compose(&world_sensor, &crate::local_mapper::inverse(&mount)?)?;
        if self.origin.is_none() {
            self.origin = Some(crate::local_mapper::inverse(&world_body)?);
        }
        let sensor_local =
            crate::local_mapper::compose(self.origin.as_ref().unwrap(), &world_sensor)?;
        let body_local = crate::local_mapper::compose(self.origin.as_ref().unwrap(), &world_body)?;
        let definition = &self.scene.voxels.definition;
        let points: Vec<_> = scan
            .local_hits()
            .into_iter()
            .map(|p| p.map(f64::from))
            .collect();
        let mut observation = DepthObservation {
            source: self.source.reference(),
            sequence,
            timestamp_ns: scan.timestamp_ns,
            clock: self.source.producer.clock.clone(),
            sensor_frame_id: definition.sources[0].sensor_frame.frame_id.clone(),
            hit_points_m: vec![],
            pose: TimedSensorPose {
                timestamp_ns: scan.timestamp_ns,
                clock: self.source.producer.clock.clone(),
                sensor_to_map: crate::local_mapper::rigid(
                    &sensor_local,
                    &definition.sources[0].sensor_frame.frame_id,
                    &self.scene.frame,
                ),
                portal_snapshot: definition.portal_snapshot.clone(),
            },
        };
        if !points.is_empty() {
            let mapper = PortalVoxelMapper::new(definition.clone())?;
            let mut rays = auki_datatypes::map::MapUpdate::default();
            // Max range is 10 m: <=303 ray samples/point. 200 rays fits the
            // SDK's 65,536 work limit. Merge batches as one scan's evidence.
            for batch in points.chunks(200) {
                observation.hit_points_m = batch.to_vec();
                rays.voxel_chunks
                    .extend(mapper.map_observation(&observation)?.voxel_chunks);
            }
            let [x, y, z, w] = sensor_local.rotation_xyzw;
            let transform = auki_datatypes::pose::SpatialTransform {
                translation: Some(auki_datatypes::pose::Vec3 {
                    x: sensor_local.translation[0],
                    y: sensor_local.translation[1],
                    z: sensor_local.translation[2],
                }),
                orientation: Some(auki_datatypes::pose::Quat { x, y, z, w }),
            };
            let endpoints = auki_mappers::Voxelizer::new(0.1, 16)?.map_sensor_points(
                points.iter().map(|p| auki_datatypes::pose::Vec3 {
                    x: p[0],
                    y: p[1],
                    z: p[2],
                }),
                &transform,
                1.,
            )?;
            let mut recent = self.recent.clone();
            let checkpoint = recent.observe(&rays, &endpoints)?;
            let mut next = self.scene.voxels.clone();
            let mut accumulator = next.accumulator()?;
            accumulator.apply(&checkpoint)?;
            next.checkpoint = accumulator.checkpoint_update().encode_to_vec();
            next.integrated_observations += 1;
            next.latest_observation = Some((&observation).into());
            next.accumulator()?;
            self.scene.voxels = next;
            self.recent = recent;
        }
        self.scene.registries = serde_json::json!({"local_mapping":{"status":"mapping","detail":"Robot-local LiDAR · perfect simulated odometry · unaligned","alignment_to_home":null,
            "robot_pose":{"capture_timestamp_ns":scan.timestamp_ns,"clock":self.source.producer.clock,"pose":body_local,"frame":self.scene.frame},
            "voxel_policy":{"unseen_cells":"retained","free_space":"measured hit rays only; no-return beams ignored","window":3}},"source_product":self.source.reference()});
        self.scene.validate(&self.scene.authority_peer_id)?;
        self.output
            .publish(self.session_clock.now_ns(), Arc::new(self.scene.clone()))?;
        ensure!(
            self.capture.errors().is_empty(),
            "Voxel scene retention failed"
        );
        Ok(())
    }
}
