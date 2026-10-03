//! Native SDK pose Product on the runner's existing peer.
use super::*;
use auki_odometry::{
    OdometryComponent, Pose, PoseContract, PoseQuery, Tracking, capture_pose_history, pose_at,
};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct MapAlignment {
    #[serde(flatten)]
    transform: auki_odometry::PoseUpdate,
    domain_id: Uuid,
    map_id: String,
}
impl ContractType for MapAlignment {
    const DATATYPE: &'static str = "galbot.map-alignment";
}
struct Mapped {
    _component: Component,
    alignment_contract: PoseContract,
    alignment_output: ConfiguredObservable<MapAlignment>,
    alignment_history: BufferProductCapture<MapAlignment>,
    domain_id: Uuid,
    map_id: String,
    alignment: Option<(u64, SpatialTransform)>,
    optical_frame: String,
}
struct State {
    sensors: std::collections::BTreeMap<String, sensors::Sensor>,
    odometry: OdometryComponent,
    history: BufferProductCapture<auki_odometry::PoseUpdate>,
    endpoint: Option<ComponentProtocolEndpoint>,
    runtime: ComponentRuntime,
    rt: tokio::runtime::Runtime,
    last_timestamp: Option<u64>,
    mapped: Option<Mapped>,
}
impl Drop for State {
    fn drop(&mut self) {
        for sensor in self.sensors.values() {
            sensor.end();
        }
        if let Some(t) = self.last_timestamp {
            let _ = self
                .odometry
                .end(t, ObservationEndReason::Reconfigured { replacement: None });
        }
        if let (Some(t), Some(m)) = (self.last_timestamp, &mut self.mapped) {
            let _ = m
                .alignment_output
                .end(t, ObservationEndReason::Reconfigured { replacement: None });
        }
        if let Some(endpoint) = self.endpoint.take() {
            let _ = self.rt.block_on(endpoint.close());
        }
    }
}
#[pyclass]
pub struct OdometryPublisher {
    state: Mutex<Option<State>>,
}
#[pymethods]
impl OdometryPublisher {
    #[staticmethod]
    fn mount(py: Python<'_>, peer: &sdk_python::PyAukiPeer, contract: &str) -> PyResult<Self> {
        let contract: PoseContract =
            serde_json::from_str(contract).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let protocols = peer.protocols();
        py.allow_threads(move || Self::create(protocols, contract))
            .map_err(PyRuntimeError::new_err)
    }
    fn publish(&self, py: Python<'_>, timestamp_ns: u64, tracking: &str) -> PyResult<()> {
        let tracking: Tracking =
            serde_json::from_str(tracking).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        py.allow_threads(|| -> Result<()> {
            let mut guard = self.state.lock().unwrap();
            let s = guard.as_mut().ok_or("odometry publisher closed")?;
            s.odometry
                .publish(timestamp_ns, tracking)
                .map_err(err)?;
            s.last_timestamp = Some(timestamp_ns);
            if !s.history.errors().is_empty() {
                return Err("pose history capture failed".into());
            }
            Ok(())
        })
        .map_err(PyRuntimeError::new_err)
    }
    fn register_sensor(&self, py: Python<'_>, specification: &str) -> PyResult<()> {
        let spec: sensors::SensorSpec = serde_json::from_str(specification)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        py.allow_threads(|| -> Result<()> {
            let mut guard = self.state.lock().unwrap();
            let state = guard.as_mut().ok_or("publisher closed")?;
            if state.sensors.contains_key(&spec.id) {
                return Err(
                    "sensor already registered; reconfiguration requires a new session".into(),
                );
            }
            let entered = state.rt.enter();
            let sensor = sensors::Sensor::mount(
                &state.runtime,
                state.endpoint.as_ref().ok_or("endpoint closed")?,
                spec,
            )?;
            drop(entered);
            state.sensors.insert(sensor.spec.id.clone(), sensor);
            Ok(())
        })
        .map_err(PyRuntimeError::new_err)
    }
    fn publish_sensor(
        &self,
        py: Python<'_>,
        sensor_id: &str,
        timestamp_ns: u64,
        data: &str,
        bytes: Vec<u8>,
    ) -> PyResult<()> {
        py.allow_threads(|| -> Result<()> {
            let mut guard = self.state.lock().unwrap();
            let state = guard.as_mut().ok_or("publisher closed")?;
            state
                .sensors
                .get_mut(sensor_id)
                .ok_or("unregistered sensor")?
                .publish(timestamp_ns, data, bytes)
        })
        .map_err(PyRuntimeError::new_err)
    }
    fn mount_mapping(
        &self,
        py: Python<'_>,
        config: &str,
        map_contract: &str,
        metadata: Py<PyAny>,
    ) -> PyResult<Pipeline> {
        let cfg: Config =
            serde_json::from_str(config).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let contract: PoseContract = serde_json::from_str(map_contract)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        py.allow_threads(|| -> Result<Pipeline> {
            let mut g = self.state.lock().unwrap();
            let s = g.as_mut().ok_or("publisher closed")?;
            let raw = s.odometry.contract();
            contract.validate().map_err(err)?;
            if s.mapped.is_some() {
                return Err("map session already mounted; restart to change alignment".into());
            }
            if contract.from_frame != raw.from_frame
                || contract.clock != raw.clock
                || contract.session_id != raw.session_id
                || cfg.capture_clock != raw.clock
                || cfg.map_frame != contract.to_frame.frame_id
                || contract.to_frame.axes != CoordinateConvention::ZUpRightForward.axes()
                || contract.to_frame.handedness
                    != CoordinateConvention::ZUpRightForward.handedness()
                || auki_geometry::meters_per_unit(contract.to_frame.units) != 1.
            {
                return Err("mapping clock/frame/session mismatch".into());
            }
            let entered = s.rt.enter();
            let endpoint = s.endpoint.as_ref().ok_or("endpoint closed")?;
            let engine = Engine::in_host(
                s.runtime.clone(),
                endpoint,
                cfg.clone(),
                Metadata::new(metadata),
            )?;
            let alignment_contract = PoseContract { from_frame: raw.to_frame.clone(), ..contract };
            alignment_contract.validate().map_err(err)?;
            let component = s.runtime.component(ComponentSpec::new("odometry-map-alignment").observable(ObservableContract {
                name: "alignment".into(), datatype: MapAlignment::DATATYPE.into(),
                schema: "galbot.map-alignment/v1".into(), access: vec![ObservationAccess::FollowNew], exposure: Exposure::Cluster,
            })).map_err(err)?;
            let alignment_output = component.configured_observable(ConfiguredObservableSpec::new(
                "alignment", format!("alignment-{}", alignment_contract.session_id), alignment_contract.clock.clone(),
                PayloadContract::Structured(StructuredPayloadContract { modality: "frame_alignment".into(),
                    datatype: MapAlignment::DATATYPE.into(), schema: "galbot.map-alignment/v1".into(),
                    observes: cfg.map_id.clone(), unit: None }),
            ).in_registered_frame(alignment_contract.to_frame.clone())).map_err(err)?;
            component.expose().map_err(err)?;
            // A pinned alignment remains applicable until this output ends. It must
            // remain retrievable by late subscribers, even after pose history expires.
            let alignment_history = s.runtime.capture_buffer(
                "odometry-map-alignment", &alignment_output, BufferLimits::entries(1),
                |o| serde_json::to_vec(o).map_or(usize::MAX, |v| v.len()),
            ).map_err(err)?;
            endpoint.export_product(&alignment_history.product()).map_err(err)?;
            drop(entered);
            s.mapped = Some(Mapped {
                _component: component,
                alignment_contract,
                domain_id: cfg.domain_id,
                map_id: cfg.map_id,
                alignment_output,
                alignment_history,
                alignment: None,
                optical_frame: cfg.optical_frame,
            });
            Ok(Pipeline {
                engine: Mutex::new(Some(engine)),
            })
        })
        .map_err(PyRuntimeError::new_err)
    }
    fn align(
        &self,
        py: Python<'_>,
        timestamp_ns: u64,
        clock: &str,
        camera_to_base: &str,
        camera_to_map: &str,
    ) -> PyResult<String> {
        let clock: ClockReference =
            serde_json::from_str(clock).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let base: RigidTransform = serde_json::from_str(camera_to_base)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let map: RigidTransform = serde_json::from_str(camera_to_map)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        py.allow_threads(|| -> Result<String> {
            let mut g = self.state.lock().unwrap(); let s = g.as_mut().ok_or("publisher closed")?;
            let raw = query(s, timestamp_ns, &clock)?;
            let m = s.mapped.as_mut().ok_or("mapping not mounted")?;
            if m.alignment.is_some() { return Err("alignment is pinned; restart for a new map session".into()); }
            check_camera(&base, &m.optical_frame, &raw.contract.from_frame.frame_id)?;
            check_camera(&map, &m.optical_frame, &m.alignment_contract.to_frame.frame_id)?;
            let camera_odom = auki_geometry::compose_spatial_transforms(&rigid_spatial(&base)?, &to_spatial(&raw.pose)).map_err(err)?;
            let alignment = auki_geometry::compose_spatial_transforms(
                &auki_geometry::inverse_spatial_transform(&camera_odom).map_err(err)?, &rigid_spatial(&map)?).map_err(err)?;
            m.alignment_output.publish(timestamp_ns, Arc::new(MapAlignment {
                transform: auki_odometry::PoseUpdate { contract: m.alignment_contract.clone(), tracking: Tracking::Valid { pose: from_spatial(&alignment) } },
                domain_id: m.domain_id, map_id: m.map_id.clone(),
            })).map_err(err)?;
            m.alignment = Some((timestamp_ns, alignment));
            serde_json::to_string(&serde_json::json!({"timestamp_ns":timestamp_ns,"clock":clock,
                "contract":&m.alignment_contract,"pose":from_spatial(&m.alignment.as_ref().unwrap().1),
                "odometry_source":raw,"product":m.alignment_history.product().reference()})).map_err(err)
        }).map_err(PyRuntimeError::new_err)
    }
    fn camera_in_map(
        &self,
        py: Python<'_>,
        timestamp_ns: u64,
        clock: &str,
        camera_to_base: &str,
    ) -> PyResult<String> {
        let clock: ClockReference =
            serde_json::from_str(clock).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let base: RigidTransform = serde_json::from_str(camera_to_base)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        py.allow_threads(|| -> Result<String> {
            let g = self.state.lock().unwrap(); let s = g.as_ref().ok_or("publisher closed")?;
            let raw = query(s, timestamp_ns, &clock)?;
            let m = s.mapped.as_ref().ok_or("mapping not mounted")?;
            check_camera(&base, &m.optical_frame, &raw.contract.from_frame.frame_id)?;
            let (start, alignment) = m.alignment.as_ref().ok_or("not calibrated")?;
            if timestamp_ns < *start { return Err("capture predates alignment".into()); }
            let cam_odom = auki_geometry::compose_spatial_transforms(&rigid_spatial(&base)?, &to_spatial(&raw.pose)).map_err(err)?;
            let pose = from_spatial(&auki_geometry::compose_spatial_transforms(&cam_odom, alignment).map_err(err)?);
            let [x,y,z,w] = pose.rotation_xyzw;
            serde_json::to_string(&serde_json::json!({"timestamp_ns":timestamp_ns,"clock":clock,"odometry_source":raw,
                "pose":RigidTransform { from_frame_id:base.from_frame_id, to_frame_id:m.alignment_contract.to_frame.frame_id.clone(),
                    translation:pose.translation, rotation_wxyz:[w,x,y,z] }})).map_err(err)
        }).map_err(PyRuntimeError::new_err)
    }
    fn status(&self) -> PyResult<String> {
        let guard = self.state.lock().unwrap();
        let s = guard
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("odometry publisher closed"))?;
        let p = s.history.product();
        serde_json::to_string(&serde_json::json!({"sensors":s.sensors.iter().map(|(id, sensor)| (id.clone(), sensor.status())).collect::<std::collections::BTreeMap<_,_>>(),"product":p.reference(),"catalog":s.runtime.catalog().snapshot(),
            "contract":s.odometry.contract(),"last_timestamp_ns":s.last_timestamp,"entries":p.buffer().range().entries,
            "aligned":s.mapped.as_ref().is_some_and(|m| m.alignment.is_some()),
            "alignment_entries":s.mapped.as_ref().map(|m|m.alignment_history.product().buffer().range().entries)}))
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }
    fn close(&self, py: Python<'_>) {
        py.allow_threads(|| drop(self.state.lock().unwrap().take()));
    }
}
impl Drop for OdometryPublisher {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap().take();
        Python::with_gil(|py| py.allow_threads(move || drop(state)));
    }
}

fn query(
    s: &State,
    timestamp_ns: u64,
    clock: &ClockReference,
) -> Result<auki_odometry::PoseAtTime> {
    if clock != &s.odometry.contract().clock {
        return Err("capture clock differs from odometry clock".into());
    }
    pose_at(
        &s.history.product(),
        &PoseQuery {
            contract: s.odometry.contract().clone(),
            timestamp_ns,
            max_gap_ns: 250_000_000,
        },
    )
    .map_err(err)
}
fn to_spatial(p: &Pose) -> SpatialTransform {
    let [x, y, z] = p.translation;
    let [qx, qy, qz, w] = p.rotation_xyzw;
    SpatialTransform {
        translation: Some(Vec3 { x, y, z }),
        orientation: Some(Quat {
            x: qx,
            y: qy,
            z: qz,
            w,
        }),
    }
}
fn from_spatial(p: &SpatialTransform) -> Pose {
    let t = p.translation.as_ref().unwrap();
    let q = p.orientation.as_ref().unwrap();
    Pose {
        translation: [t.x, t.y, t.z],
        rotation_xyzw: [q.x, q.y, q.z, q.w],
    }
}
fn rigid_spatial(p: &RigidTransform) -> Result<SpatialTransform> {
    let [w, x, y, z] = p.rotation_wxyz;
    let pose = Pose {
        translation: p.translation,
        rotation_xyzw: [x, y, z, w],
    };
    pose.validate().map_err(err)?;
    Ok(to_spatial(&pose))
}
fn check_camera(p: &RigidTransform, from: &str, to: &str) -> Result<()> {
    if p.from_frame_id != from || p.to_frame_id != to {
        return Err("camera transform frame mismatch".into());
    }
    rigid_spatial(p)?;
    Ok(())
}

impl OdometryPublisher {
    fn create(protocols: ::auki_sdk::AukiPeerProtocols, contract: PoseContract) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new().map_err(err)?;
        let entered = rt.enter();
        let runtime = ComponentRuntime::new(protocols.peer_id().to_string());
        let odometry = OdometryComponent::new(&runtime, "odometry", contract).map_err(err)?;
        let history = capture_pose_history(
            &runtime,
            "robot-odometry-history",
            &odometry,
            BufferLimits {
                max_entries: Some(12_000),
                max_bytes: Some(32 * 1024 * 1024),
                target_duration: Some(Duration::from_secs(10)),
            },
        )
        .map_err(err)?;
        let endpoint = ComponentProtocolEndpoint::mount(protocols, runtime.clone()).map_err(err)?;
        endpoint.export_product(&history.product()).map_err(err)?;
        drop(entered);
        Ok(Self {
            state: Mutex::new(Some(State {
                sensors: Default::default(),
                odometry,
                history,
                endpoint: Some(endpoint),
                runtime,
                rt,
                last_timestamp: None,
                mapped: None,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{image, metadata, support};
    use ::auki_sdk::{AukiPeer, Identity};
    use auki_component_protocol::{
        CatalogResponse, ComponentProtocolClient, ObservationStart, RemoteObservationEvent,
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_peer_composes_local_odometry_and_alignment_without_a_mapped_pose_product() {
        let domain = Uuid::new_v4();
        let i1 = Identity::generate();
        let i2 = Identity::generate();
        let (p1, _a1) = AukiPeer::start_external(
            i1.clone(),
            support::authority(&i1, domain),
            support::direct_config(),
        )
        .await
        .unwrap();
        let (p2, _a2) = AukiPeer::start_external(
            i2.clone(),
            support::authority(&i2, domain),
            support::direct_config(),
        )
        .await
        .unwrap();
        let owner = p1.peer_id().to_string();
        let contract: PoseContract=serde_json::from_value(serde_json::json!({
            "from_frame":{"peer_id":owner,"frame_id":"robot.base","axes":{"x":"forward","y":"left","z":"up"},"units":"meters","handedness":"right"},
            "to_frame":{"peer_id":owner,"frame_id":"odom-test","axes":{"x":"forward","y":"left","z":"up"},"units":"meters","handedness":"right"},
            "clock":{"peer_id":owner,"id":"sensor-time","hash":"a".repeat(32)},"session_id":"fixture"
        })).unwrap();
        let clock = contract.clock.clone();
        let protocols = p1.protocols();
        let (publisher,pipeline)=tokio::task::spawn_blocking(move || {
            let publisher=OdometryPublisher::create(protocols,contract.clone()).unwrap();
            let cfg=Config {domain_id:domain,map_id:"observed".into(),map_frame:"domain-zup".into(),optical_frame:"optical".into(),optical_registry:FrameRegistryEntry::ros_optical(clock.peer_id.clone(),"optical"),
                capture_clock:clock.clone(),publication_clock:clock.clone(),width:800,height:600,fx:500.,fy:500.,cx:400.,cy:300.};
            Python::with_gil(|py| {
                let mut mapped=contract.clone(); mapped.to_frame.frame_id="domain-zup".into(); mapped.to_frame.axes=CoordinateConvention::ZUpRightForward.axes();
                let cfg_json=serde_json::json!({"domain_id":domain,"map_id":cfg.map_id,"map_frame":cfg.map_frame,
                    "optical_frame":cfg.optical_frame,"optical_registry":cfg.optical_registry,"capture_clock":clock,"publication_clock":clock,
                    "width":800,"height":600,"fx":500.,"fy":500.,"cx":400.,"cy":300.}).to_string();
                let pipeline=publisher.mount_mapping(py,&cfg_json,&serde_json::to_string(&mapped).unwrap(), metadata().0.as_ref().clone_ref(py)).unwrap();
                for (t,x) in [(100,2.),(300,4.)] {
                    publisher.publish(py,t,&serde_json::to_string(&Tracking::Valid{pose:Pose{translation:[x,0.,0.],rotation_xyzw:[0.,0.,0.,1.]}}).unwrap()).unwrap();
                }
                let camera=RigidTransform {from_frame_id:"optical".into(),to_frame_id:"domain-zup".into(),translation:[10.,20.,1.],rotation_wxyz:[std::f64::consts::FRAC_1_SQRT_2,0.,0.,std::f64::consts::FRAC_1_SQRT_2]};
                let id=Uuid::from_u128(1);
                let empty:serde_json::Value=serde_json::from_str(&pipeline.localize(py,image(id),50).unwrap()).unwrap();
                assert!(empty.get("camera_to_map").is_none());
                pipeline.process(py,image(id),100,&serde_json::to_string(&camera).unwrap()).unwrap();
                let mut anchors: Vec<_>=pipeline.engine.lock().unwrap().as_ref().unwrap().map.product().latest_existing().unwrap().unwrap().payload.scenegraph.anchors.values().cloned().collect();
                anchors[0].payload="HTTPS://R8.HR/FNPUME0T93U".into();
                pipeline.seed_reference(py,&serde_json::to_string(&anchors).unwrap()).unwrap();
                let result:serde_json::Value=serde_json::from_str(&pipeline.localize(py,image(id),200).unwrap()).unwrap();
                let solved:RigidTransform=serde_json::from_value(result["camera_to_map"].clone()).unwrap();
                for i in 0..3 { assert!((solved.translation[i]-camera.translation[i]).abs()<0.01); }
                let base=RigidTransform {from_frame_id:"optical".into(),to_frame_id:"robot.base".into(),translation:[0.2,0.,0.5],rotation_wxyz:[1.,0.,0.,0.]};
                let bj=serde_json::to_string(&base).unwrap();let cj=serde_json::to_string(&clock).unwrap();
                publisher.align(py,200,&cj,&bj,&result["camera_to_map"].to_string()).unwrap();
                // Both bracketing samples exist; midpoint is x=3, not latest x=4.
                let mapped:serde_json::Value=serde_json::from_str(&publisher.camera_in_map(py,200,&cj,&bj).unwrap()).unwrap();
                for i in 0..3 { assert!((mapped["pose"]["translation"][i].as_f64().unwrap()-camera.translation[i]).abs()<0.01); }
                assert!(publisher.camera_in_map(py,99,&cj,&bj).is_err());
                let mut wrong=clock.clone();wrong.id="other-clock".into();
                assert!(publisher.camera_in_map(py,200,&serde_json::to_string(&wrong).unwrap(),&bj).is_err());
                publisher.publish(py,400,&serde_json::to_string(&Tracking::Valid{pose:Pose{translation:[5.,0.,0.],rotation_xyzw:[0.,0.,0.,1.]}}).unwrap()).unwrap();
                let camera_mapped:serde_json::Value=serde_json::from_str(&publisher.camera_in_map(py,400,&cj,&bj).unwrap()).unwrap();
                pipeline.process(py,image(Uuid::from_u128(2)),400,&camera_mapped["pose"].to_string()).unwrap();
                // Advance beyond the ten-second history window before a peer connects.
                publisher.publish(py,11_000_000_400,&serde_json::to_string(&Tracking::Valid{pose:Pose{translation:[5.,0.,0.],rotation_xyzw:[0.,0.,0.,1.]}}).unwrap()).unwrap();
                (publisher,pipeline)
            })
        }).await.unwrap();
        let client = ComponentProtocolClient::new(p2.protocols());
        let route = p1.listen_addresses()[0].clone();
        let CatalogResponse::Snapshot { snapshot: catalog } = client
            .catalog_exact(p1.peer_id(), route.clone(), None)
            .await
            .unwrap()
        else {
            panic!("catalog")
        };
        let product = catalog
            .products
            .iter()
            .find(|p| p.manifest.product_id == "robot-odometry-history")
            .unwrap();
        let reference = publisher
            .state
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .history
            .product()
            .reference();
        assert_eq!(
            product.metadata.as_ref().unwrap().value["to_frame"]["frame_id"],
            "odom-test"
        );
        assert!(!catalog.products.iter().any(|p| p.manifest.product_id == "robot-map-pose-history"));
        let mut remote = client
            .subscribe_product_exact::<auki_odometry::PoseUpdate>(
                p1.peer_id(),
                route.clone(),
                reference,
                ObservationStart::LatestExisting,
                BufferLimits::entries(4),
                |_| 2048,
            )
            .await
            .unwrap();
        let RemoteObservationEvent::Observation(o) = remote.next().await.unwrap().unwrap() else {
            panic!("pose")
        };
        assert_eq!(o.timestamp_ns, 11_000_000_400);
        assert_eq!(publisher.state.lock().unwrap().as_ref().unwrap().history.product().buffer().range().entries, 1);
        let Tracking::Valid { pose } = &o.payload.tracking else {
            panic!("tracking")
        };
        assert_eq!(pose.translation, [5., 0., 0.]);
        assert_eq!(o.payload.contract.to_frame.frame_id, "odom-test");
        let alignment_ref = publisher.state.lock().unwrap().as_ref().unwrap().mapped.as_ref().unwrap().alignment_history.product().reference();
        let mut alignments = client.subscribe_product_exact::<MapAlignment>(
            p1.peer_id(), route.clone(), alignment_ref, ObservationStart::LatestExisting,
            BufferLimits::entries(1), |_| 2048,
        ).await.unwrap();
        let RemoteObservationEvent::Observation(a) = alignments.next().await.unwrap().unwrap() else { panic!("alignment") };
        assert_eq!(a.timestamp_ns, 200);
        assert_eq!(a.payload.domain_id, domain);
        assert_eq!(a.payload.map_id, "observed");
        assert_eq!(a.payload.transform.contract.from_frame, o.payload.contract.to_frame);
        assert_eq!(a.payload.transform.contract.clock, o.payload.contract.clock);
        assert_eq!(a.payload.transform.contract.session_id, o.payload.contract.session_id);
        let Tracking::Valid {pose: alignment} = &a.payload.transform.tracking else {panic!("alignment lost")};
        // Only the consuming peer composes the robot's local pose into map space.
        let composed = from_spatial(&auki_geometry::compose_spatial_transforms(&to_spatial(pose), &to_spatial(alignment)).unwrap());
        assert!((composed.translation[0] - 10.).abs() < 0.01);
        assert!((composed.translation[1] - 21.8).abs() < 0.01);
        assert!((composed.translation[2] - 0.5).abs() < 0.01);
        let mapref = pipeline
            .engine
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .map
            .product()
            .reference();
        let mut maps = client
            .subscribe_product_exact::<MapSnapshot>(
                p1.peer_id(),
                route,
                mapref,
                ObservationStart::LatestExisting,
                BufferLimits::entries(2),
                MapSnapshot::encoded_size,
            )
            .await
            .unwrap();
        let RemoteObservationEvent::Observation(map) = maps.next().await.unwrap().unwrap() else {
            panic!("map")
        };
        assert_eq!(
            map.payload.scenegraph.map.frame.id,
            a.payload.transform.contract.to_frame.frame_id
        );
        assert_eq!(map.payload.scenegraph.anchors.len(), 2);
        alignments.close().await.unwrap();
        remote.close().await.unwrap();
        maps.close().await.unwrap();
        tokio::task::spawn_blocking(move || {
            Python::with_gil(|py| {
                pipeline.close(py);
                publisher.close(py);
            })
        })
        .await
        .unwrap();
        p2.shutdown().await.unwrap();
        p1.shutdown().await.unwrap();
    }
}
