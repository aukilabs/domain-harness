mod support;
use anyhow::{Context, Result};
use auki_component_protocol::RemoteObservationEvent;
use spatial_home::{
    model::{Event, Store, clock_ref, fixtures},
    network::{HomeConnection, ScenePublisher, subscribe_scene},
};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_scene_streams_over_authenticated_tcp_and_rebinds_after_publication_restart()
-> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let mut store = Store::open(dir.path().join("home.json"))?;
        let mut publisher = ScenePublisher::mount(&pair.a)?;
        assert!(publisher.publish(&store.built)?);
        let connection = HomeConnection::direct(&pair.a);
        let mut subscription = subscribe_scene(&pair.b, &connection).await?;
        let product = subscription.product().reference();
        let event = tokio::time::timeout(Duration::from_secs(10), subscription.next()).await??;
        let Some(RemoteObservationEvent::Observation(first)) = event else {
            panic!("no initial scene")
        };
        first.payload.validate(&connection.peer_id)?;
        assert_eq!(first.payload.portals.scenegraph.anchors.len(), 14);
        let landmarks = spatial_home::landmarks::from_registries(
            &first.payload.registries,
            &first.payload.frame,
        )?;
        assert_eq!(landmarks.anchors.len(), 13);
        assert_eq!(
            landmarks.by_type(spatial_home::landmarks::PORTAL).count(),
            52
        );
        assert!(
            serde_json::to_value(&first.payload)?
                .get("revision")
                .is_none()
        );
        assert_eq!(
            clock_ref(&first.payload.publication_clock),
            subscription.product().producer.clock
        );
        assert!(!publisher.publish(&store.built)?);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), subscription.next())
                .await
                .is_err()
        );

        let fixture = fixtures()?;
        store.apply(Event::Contribution(serde_json::from_value(
            fixture["contributions"][0].clone(),
        )?))?;
        assert!(publisher.publish(&store.built)?);
        let Some(RemoteObservationEvent::Observation(second)) =
            tokio::time::timeout(Duration::from_secs(10), subscription.next()).await??
        else {
            panic!("no scene update")
        };
        second.payload.validate(&connection.peer_id)?;
        assert!(
            serde_json::to_value(&second.payload)?
                .get("revision")
                .is_none()
        );
        assert!(second.payload.voxels.integrated_observations > 0);
        assert_eq!(subscription.product().reference(), product);
        assert_eq!(second.sequence, first.sequence + 1);

        // Neither a claimed payload authority nor a renamed coordinate frame is accepted.
        let mut forged = (*second.payload).clone();
        forged.authority_peer_id = pair.b.peer_id().to_string();
        assert!(forged.validate(&connection.peer_id).is_err());
        let mut wrong_frame = (*second.payload).clone();
        wrong_frame.frame = "robot-local".into();
        assert!(wrong_frame.validate(&connection.peer_id).is_err());

        subscription.close().await?;
        publisher.close().await?;
        let mut restarted = ScenePublisher::mount(&pair.a)?;
        restarted.publish(&store.built)?;
        let mut rebound = subscribe_scene(&pair.b, &connection).await?;
        assert_ne!(
            rebound.product().reference().manifest_hash,
            product.manifest_hash
        );
        let Some(RemoteObservationEvent::Observation(latest)) =
            tokio::time::timeout(Duration::from_secs(10), rebound.next()).await??
        else {
            panic!("no resumed scene")
        };
        assert!(
            serde_json::to_value(&latest.payload)?
                .get("revision")
                .is_none()
        );
        rebound.close().await?;
        restarted.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_rejects_wrong_domain_and_expected_peer() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let publisher = ScenePublisher::mount(&pair.a)?;
        let mut connection = HomeConnection::direct(&pair.a);
        connection.domain_id = uuid::Uuid::new_v4().to_string();
        assert!(subscribe_scene(&pair.b, &connection).await.is_err());
        connection.domain_id = pair.a.domain_id().to_string();
        connection.peer_id = auki_sdk::Identity::generate().peer_id().to_string();
        assert!(subscribe_scene(&pair.b, &connection).await.is_err());
        publisher.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_observations_build_independent_owned_maps_and_survive_replay() -> Result<()> {
    use auki_components::ProductReference;
    use auki_odometry::Pose;
    use auki_registry::FrameRegistryEntry;
    use auki_voxel_map::SensorBinding;
    use spatial_home::{
        model::clock_entry,
        runner_voxels::{LidarConfig, PreparedScan},
        shared_inputs,
    };
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let mut home = Store::open_live(dir.path().join("home.json"))?;
        let robot_path = dir.path().join("robot.json");
        let robot_id = pair.b.peer_id().to_string();

        let source = SensorBinding {
            sensor_product: ProductReference {
                peer_id: robot_id.clone(),
                product_id: "lidar.base-history".into(),
                manifest_hash: "fixture".into(),
            },
            sensor_frame: FrameRegistryEntry::ros_body(&robot_id, "lidar_base_link"),
            observation_clock: clock_ref(&clock_entry(&robot_id, "capture")),
        };
        let pose = Pose {
            translation: [-4.035, 0.335, 4.035],
            rotation_xyzw: [0., 0., 0., 1.],
        };
        let scan = PreparedScan {
            hit_colors_srgb: vec![Some([255, 0, 0]), Some([0, 0, 255])],
            source,
            sequence: 7,
            timestamp_ns: 7_000_000_000,
            hit_points_m: vec![[1., 0., 0.], [1., 0.2, 0.]],
            sensor_to_home: pose,
            alignment_key: "fixture".into(),
            calibration: LidarConfig {
                sensor_frame: "lidar_base_link".into(),
                sensor_to_base: Pose {
                    translation: [0.; 3],
                    rotation_xyzw: [0., 0., 0., 1.],
                },
                calibration_source: "fixture".into(),
            },
        };
        home.integrate_prepared(scan)?;
        let mut home_pub = ScenePublisher::mount(&pair.a)?;
        home_pub.publish(&home.built)?;
        let mut portal_subscription =
            subscribe_scene(&pair.b, &HomeConnection::direct(&pair.a)).await?;
        let Some(RemoteObservationEvent::Observation(reference)) =
            portal_subscription.next().await?
        else {
            panic!("Home portal reference missing")
        };
        let mut robot = Store::open_mapper(&robot_path, &reference.payload.portals, &robot_id)?;
        portal_subscription.close().await?;
        home_pub.publish_inputs(home.mapping_inputs())?;
        let mut inputs =
            shared_inputs::subscribe(&pair.b, &HomeConnection::direct(&pair.a)).await?;
        let Some(RemoteObservationEvent::Observation(input)) =
            tokio::time::timeout(Duration::from_secs(10), inputs.next())
                .await
                .context("shared inputs timeout")??
        else {
            panic!("no shared input")
        };
        assert!(robot.integrate_prepared(input.payload.scan.clone())?);
        assert_ne!(home.built.view.frame, robot.built.view.frame);
        assert!(spatial_home::conventions::matching_portals(
            &robot.built.portals,
            &home.built.portals
        )?);
        assert_eq!(robot.built.voxels.definition.registry.peer_id, robot_id);
        assert_eq!(
            robot.built.voxels.definition.sources,
            home.built.voxels.definition.sources
        );
        let mut robot_pub = ScenePublisher::mount(&pair.b)?;
        robot_pub.publish(&robot.built)?;
        let mut connection = HomeConnection::direct(&pair.b);
        connection.frame = spatial_home::conventions::RUNNER_FRAME.into();
        let mut remote = subscribe_scene(&pair.a, &connection).await?;
        let Some(RemoteObservationEvent::Observation(scene)) =
            tokio::time::timeout(Duration::from_secs(10), remote.next())
                .await
                .context("remote scene timeout")??
        else {
            panic!("no independent scene")
        };
        scene.payload.validate(&robot_id)?;
        assert_eq!(scene.payload.portals.scenegraph.anchors.len(), 13);
        let rendered = spatial_home::scene_mirror::render_scene(&scene.payload)?;
        let cells: Vec<spatial_home::raycast::Voxel> =
            serde_json::from_value(rendered["voxels"].clone())?;
        assert_eq!(cells.len(), home.built.view.voxels.len());
        for cell in &cells {
            let home_cell = home
                .built
                .view
                .voxels
                .iter()
                .find(|v| {
                    v.center
                        .iter()
                        .zip(cell.center)
                        .all(|(a, b)| (a - b).abs() < 1e-9)
                })
                .expect("same physical voxel");
            assert_eq!(cell.linear_rgb, home_cell.linear_rgb);
            assert!(cell.linear_rgb.is_some());
        }
        assert_eq!(rendered["frame"], spatial_home::model::FRAME);
        assert_eq!(scene.payload.frame, spatial_home::conventions::RUNNER_FRAME);
        let mut restored = Store::open_mapper(&robot_path, &home.built.portals, &robot_id)?;
        restored.set_map_owner(&robot_id)?;
        assert!(!restored.integrate_prepared(input.payload.scan.clone())?);
        assert!(
            serde_json::to_value(&restored.built.view)?
                .get("revision")
                .is_none()
        );
        assert_eq!(restored.mapping_inputs().len(), 1);
        inputs.close().await?;
        remote.close().await?;
        home_pub.close().await?;
        robot_pub.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn rgbd_component_preserves_capture_clock_color_and_calibration_over_p2p() -> Result<()> {
    use spatial_home::{rgbd, runner_voxels::SensorCalibration};
    let pair = support::LocalPair::start().await?;
    let result = async {
        let publisher = ScenePublisher::mount(&pair.b)?;
        let clock = clock_ref(&spatial_home::model::clock_entry(
            "physical-galbot",
            "capture",
        ));
        let source = rgbd::Publisher::mount(
            &publisher.runtime,
            &publisher.endpoint,
            &pair.b.peer_id().to_string(),
            clock.clone(),
        )?;
        let sample = rgbd::Capture {
            timestamp_ns: 100,
            rgb_timestamp_ns: 100,
            transform_timestamp_ns: 100,
            hit_points_m: vec![[0.1, 0., 0.3]],
            hit_colors_srgb: vec![[12, 140, 211]],
            calibration: SensorCalibration {
                sensor_frame: rgbd::FRAME.into(),
                sensor_to_base: auki_odometry::Pose {
                    translation: [0.2, 0., 0.6],
                    rotation_xyzw: [0., 0., 0., 1.],
                },
                calibration_source:
                    "Galbot SDK D405 co-registered RGB-D; RealSense deprojection; sha256:fixture"
                        .into(),
            },
        };
        source.publish(sample.clone())?;
        let connection = HomeConnection::direct(&pair.b);
        let client = auki_component_protocol::ComponentProtocolClient::new(pair.a.protocols());
        let received = rgbd::fetch(&client, Some(&connection)).await?;
        assert_eq!(received.producer.clock, clock);
        assert_eq!(received.observations[0].timestamp_ns, 100);
        assert_eq!(
            serde_json::to_value(&*received.observations[0].payload)?,
            serde_json::to_value(sample)?
        );
        publisher.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn robot_occupancy_product_replaces_cells_and_expires_without_refreshing_reads() -> Result<()>
{
    use auki_components::{ContractType, PayloadContract, StructuredPayloadContract};
    use spatial_home::{
        galbot_sensors::{Sensor, SensorSpec},
        robot_occupancy::{self, RobotOccupancy},
    };
    let pair = support::LocalPair::start().await?;
    let result=async {
        let publisher=ScenePublisher::mount(&pair.a)?;
        let owner=pair.a.peer_id().to_string();
        let clock=clock_ref(&spatial_home::model::clock_entry(&owner,"body-test"));
        let mut body=RobotOccupancy {session_id:"body-test".into(),model_sha256:"a".repeat(64),voxel_size_m:0.05,
            grid_origin_m:[0.;3],cells:vec![[1,2,3]],covered_links:vec!["base".into()],missing_links:vec![],approximate_links:vec![],compute_ms:1.};
        let mut sensor=Sensor::mount(&publisher.runtime,&publisher.endpoint,SensorSpec {id:"robot.occupancy".into(),clock,
            frame_definition:Some(auki_registry::FrameRegistryEntry::ros_body(&owner,"robot.base")),
            payload:PayloadContract::Structured(StructuredPayloadContract {datatype:RobotOccupancy::DATATYPE.into(),schema:"spatial.robot-occupancy/v1".into(),modality:"robot_occupancy".into(),observes:"robot".into(),unit:Some("meters".into())}),
            metadata:serde_json::json!({"model_sha256":body.model_sha256,"session_id":body.session_id}),
        }).map_err(anyhow::Error::msg)?;
        sensor.publish(100,&serde_json::to_string(&body)?,vec![]).map_err(anyhow::Error::msg)?;
        let cache=robot_occupancy::Cache::default();let stop=tokio_util::sync::CancellationToken::new();
        let targets=[HomeConnection::direct(&pair.a)];
        let monitor=robot_occupancy::monitor(&pair.b,&targets,&cache,&stop);
        let verify=async {
            let checks=async {
                while cache.lock().unwrap().get(&owner).is_none(){tokio::time::sleep(Duration::from_millis(50)).await;}
                let received=cache.lock().unwrap()[&owner].back().unwrap().received;
                tokio::time::sleep(Duration::from_millis(700)).await;
                assert_eq!(cache.lock().unwrap()[&owner].back().unwrap().received,received);
                body.cells=vec![[9,8,7]];
                sensor.publish(200,&serde_json::to_string(&body)?,vec![]).map_err(anyhow::Error::msg)?;
                while cache.lock().unwrap()[&owner].back().unwrap().timestamp_ns!=200{tokio::time::sleep(Duration::from_millis(50)).await;}
                assert_eq!(cache.lock().unwrap()[&owner].back().unwrap().body.cells,vec![[9,8,7]]);
                sensor.end();
                while cache.lock().unwrap().contains_key(&owner){tokio::time::sleep(Duration::from_millis(50)).await;}
                Ok::<_,anyhow::Error>(())
            };
            let result=tokio::time::timeout(Duration::from_secs(15),checks).await;
            stop.cancel();result?
        };
        let (_,result)=tokio::join!(monitor,verify);
        publisher.close().await?;
        result
    }.await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}
