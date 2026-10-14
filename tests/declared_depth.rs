#[allow(dead_code)]
mod support;
use anyhow::Result;
use auki_components::*;
use spatial_home::{
    compact_pose::{Contract, Lifetime, Update},
    declared_depth::{Points, PointsContract},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn home_integrates_declared_camera_points_over_authenticated_sdk() -> Result<()> {
    run(false, false, false).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn home_consumes_the_actual_mobile_declared_publisher() -> Result<()> {
    run(true, false, false).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn home_voxelizes_points_through_sensor_mount_odometry_and_home_alignment() -> Result<()> {
    run(false, true, false).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn home_subscribes_to_empty_compact_depth_and_preserves_capture_chain() -> Result<()> {
    run(false, true, true).await
}
async fn run(actual_publisher: bool, extra_mount: bool, packed: bool) -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let scenes =
            std::sync::Arc::new(spatial_home::local_scenes::LocalScenes::load(dir.path())?);
        let home = pair.a.peer_id().to_string();
        let phone = pair.b.peer_id().to_string();
        let server = spatial_home::network::ScenePublisher::mount(&pair.a)?;
        let mut publishers = spatial_home::local_mobile::Publisher::mount_all(
            &server.runtime,
            &server.endpoint,
            scenes.clone(),
            spatial_home::model::clock_entry(&home, "home-session"),
            std::time::Instant::now(),
        )?;
        let frame = scenes
            .snapshot("humanoid-hub-conference")?
            .scenegraph
            .map
            .frame
            .id
            .clone();
        let definition = spatial_home::conventions::registry(&home, &frame)?;
        let definitions = spatial_home::product_registry::Definitions::default();
        spatial_home::product_registry::frame(&definitions, &definition)?;
        let _registry = spatial_home::product_registry::mount(
            &pair.a,
            server.runtime.clone(),
            definitions.clone(),
        )?;
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let session = uuid::Uuid::new_v4().to_string();
        let sample = spatial_home::mobile_pose::Sample {
            session: session.clone(),
            timestamp_ns: 100,
            camera_configuration_id: None,
            pose: auki_scenegraph::RigidTransform {
                from_frame_id: format!("phone-camera:{session}"),
                to_frame_id: format!("phone-ar:{session}"),
                translation: [0., 1., 0.],
                rotation_wxyz: [1., 0., 0., 0.],
            },
        };
        let mut native = spatial_home::mobile_odometry::Publisher::new(&products, &home, &sample)?;
        native.publish(&home, &sample)?;
        let catalog = products.runtime.catalog().snapshot();
        let pc: Contract =
            serde_json::from_value(catalog.products[0].metadata.as_ref().unwrap().value.clone())?;
        let point_frame = auki_registry::FrameRegistryEntry::opengl(&phone,
            if extra_mount {"arbitrary-depth-lens"} else {&pc.from_frame.id});
        let _mount = if extra_mount {
            use auki_protocols::registry::v3::{RegistryEntryEnvelope,RegistryKind};
            products.definitions.lock().unwrap().push(RegistryEntryEnvelope {kind:RegistryKind::Frame,
                id:point_frame.frame_id.clone(),hash:point_frame.hash(),canonical_json:String::from_utf8(point_frame.canonical_bytes())?});
            let c=Contract {from_frame:(&point_frame).into(),to_frame:pc.from_frame.clone(),lifetime:Lifetime::Held,..pc.clone()};
            let component=products.runtime.component(ComponentSpec::new("arbitrary-mount").observable(ObservableContract {
                name:"mount".into(),datatype:Update::DATATYPE.into(),schema:Update::DATATYPE.into(),access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster}))?;
            let output=component.configured_observable(ConfiguredObservableSpec::new("mount",c.output_id(),c.clock.clone(),
                PayloadContract::Structured(StructuredPayloadContract {datatype:Update::DATATYPE.into(),schema:Update::DATATYPE.into(),modality:"pose".into(),observes:"mount".into(),unit:None}))
                .in_registered_frame(auki_registry::FrameRegistryEntry::opengl(&phone,&pc.from_frame.id)))?;
            component.expose()?;
            let retained=products.runtime.capture_buffer_with_metadata("arbitrary-mount-history",&output,
                BufferLimits {max_entries:Some(4),max_bytes:None,target_duration:None},|_:&Update|256,
                move |o|Ok(Some(CatalogProductMetadata {schema:spatial_home::compact_pose::METADATA.into(),source_sequence:o.sequence,value:serde_json::to_value(&c).unwrap()})))?;
            products.endpoint.export_product(&retained.product())?;
            output.publish(0,std::sync::Arc::new(Update {tracking:auki_odometry::Tracking::Valid {pose:auki_odometry::Pose {translation:[1.,0.,0.],rotation_xyzw:[0.,0.,0.,1.]}}}))?;
            Some((component,output,retained))
        } else {None};
        let mut packed_send = None;
        let mut phone_depth = None;
        let _retained: Box<dyn std::any::Any> = if actual_publisher {
            let mut publisher = spatial_home::mobile_declared::Publisher::new(&products, &session, &home, &frame)?;
            publisher.align(1, &auki_scenegraph::RigidTransform {
                from_frame_id: pc.to_frame.id.clone(), to_frame_id: frame.clone(),
                translation: [2.,0.,0.], rotation_wxyz: [1.,0.,0.,0.]
            })?;
            // The phone's depth is binary and Home follows it from when it subscribes,
            // so the capture goes out once Home is listening.
            phone_depth = Some(publisher);
            Box::new(())
        } else {
        let held = Contract {
            from_frame: pc.to_frame.clone(),
            to_frame: (&definition).into(),
            lifetime: Lifetime::Held,
            ..pc.clone()
        };
        let component = products.runtime.component(
            ComponentSpec::new("arbitrary-transform-name").observable(ObservableContract {
                name: "output".into(),
                datatype: spatial_home::compact_pose::SCHEMA.into(),
                schema: spatial_home::compact_pose::SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }),
        )?;
        let output: ConfiguredObservable<Update> = component.configured_observable(
            ConfiguredObservableSpec::new(
                "output",
                held.output_id(),
                held.clock.clone(),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "pose".into(),
                    datatype: spatial_home::compact_pose::SCHEMA.into(),
                    schema: spatial_home::compact_pose::SCHEMA.into(),
                    observes: "declared_transform".into(),
                    unit: None,
                }),
            )
            .in_registered_frame(definition),
        )?;
        component.expose()?;
        let descriptor = held.clone();
        let capture = products.runtime.capture_buffer_with_metadata(
            "arbitrary-held-product",
            &output,
            BufferLimits {
                max_entries: Some(8),
                max_bytes: Some(65536),
                target_duration: None,
            },
            |_: &Update| 256,
            move |o| {
                Ok(Some(CatalogProductMetadata {
                    schema: spatial_home::compact_pose::METADATA.into(),
                    source_sequence: o.sequence,
                    value: serde_json::to_value(&descriptor).unwrap(),
                }))
            },
        )?;
        products.endpoint.export_product(&capture.product())?;
        output.publish(
            1,
            Update {
                tracking: auki_odometry::Tracking::Valid {
                    pose: auki_odometry::Pose {
                        translation: [2., 0., 0.],
                        rotation_xyzw: [0., 0., 0., 1.],
                    },
                },
            }
            .into(),
        )?;
        let dc = PointsContract {
            frame: (&point_frame).into(),
            clock: pc.clock.clone(),
            session_id: session.clone(),
        };
        if packed {
            use spatial_home::depth_points::{PackedPoints, PACKED_POINTS_SCHEMA};
            let component = products.runtime.component(ComponentSpec::new("arbitrary-binary-depth")
                .observable(ObservableContract {name:"points".into(), datatype:PACKED_POINTS_SCHEMA.into(),
                    schema:PACKED_POINTS_SCHEMA.into(), access:vec![ObservationAccess::FollowNew], exposure:Exposure::Cluster}))?;
            let depth: ConfiguredObservable<PackedPoints> = component.configured_observable(
                ConfiguredObservableSpec::new("points", dc.output_id(), dc.clock.clone(),
                    PayloadContract::Structured(StructuredPayloadContract {modality:"depth".into(),
                        datatype:PACKED_POINTS_SCHEMA.into(), schema:PACKED_POINTS_SCHEMA.into(),
                        observes:"measured_points".into(), unit:Some("meters".into())}))
                    .in_registered_frame(point_frame.clone()))?;
            component.expose()?;
            let retained = products.runtime.capture_buffer("empty-until-subscribed", &depth,
                BufferLimits {max_entries:Some(1),max_bytes:Some(128*1024),target_duration:None},
                |p:&PackedPoints|p.as_bytes().len())?;
            products.endpoint.export_product_with_encoder(&retained.product(), PackedPoints::encoder()?)?;
            // There is deliberately no metadata or initial observation. A consumer
            // discovers the exact typed Output, not a robot name or sentinel cloud.
            assert!(retained.product().latest_existing()?.is_none());
            assert_eq!(products.endpoint.product_active_subscriptions("empty-until-subscribed"),0);
            let packet = PackedPoints::from_points(dc, &Points {points:vec![[0.,-1.,-1.]], colors:Some(vec![[255,0,0]])})?;
            packed_send = Some((depth, packet));
            Box::new((component, output, capture, retained))
        } else {
        let component = products.runtime.component(
            ComponentSpec::new("arbitrary-depth-component").observable(ObservableContract {
                name: "points".into(),
                datatype: spatial_home::declared_depth::SCHEMA.into(),
                schema: spatial_home::declared_depth::SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }),
        )?;
        let depth: ConfiguredObservable<Points> = component.configured_observable(
            ConfiguredObservableSpec::new(
                "points",
                format!("points-{}", manifest_hash(&dc)),
                dc.clock.clone(),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "depth".into(),
                    datatype: spatial_home::declared_depth::SCHEMA.into(),
                    schema: spatial_home::declared_depth::SCHEMA.into(),
                    observes: "camera_points".into(),
                    unit: Some("meters".into()),
                }),
            )
            .in_registered_frame(point_frame.clone()),
        )?;
        component.expose()?;
        let retained = products.runtime.capture_buffer_with_metadata(
            "not-named-depth-points",
            &depth,
            BufferLimits {
                max_entries: Some(4),
                max_bytes: Some(65536),
                target_duration: None,
            },
            |_: &Points| 256,
            move |o| {
                Ok(Some(CatalogProductMetadata {
                    schema: spatial_home::declared_depth::METADATA.into(),
                    source_sequence: o.sequence,
                    value: serde_json::to_value(&dc).unwrap(),
                }))
            },
        )?;
        products.endpoint.export_product(&retained.product())?;
        depth.publish(
            100,
            Points {
                points: vec![[0., -1., -1.]],
                colors: Some(vec![[255, 0, 0]]),
            }
            .into(),
        )?;
            Box::new((component, output, capture, depth, retained))
        }
        };
        let camera_component = products.runtime.component(ComponentSpec::new("ar-test-camera")
            .observable(ObservableContract { name:"rgb".into(), datatype:VideoFrame::DATATYPE.into(),
                schema:"auki.video-frame/v1".into(), access:vec![ObservationAccess::FollowNew], exposure:Exposure::Cluster }))?;
        let camera: ConfiguredObservable<VideoFrame> = camera_component.configured_observable(
            ConfiguredObservableSpec::new("rgb","camera",pc.clock.clone(),PayloadContract::Camera(CameraPayloadContract {
                datatype:VideoFrame::DATATYPE.into(),schema:"auki.video-frame/v1".into(),encoding:"jpeg".into(),
                width:640,height:480,nominal_frame_rate_hz:Some(10),observes:"visible_light".into()
            })).in_registered_frame(auki_registry::FrameRegistryEntry::opengl(&phone,&pc.from_frame.id)))?;
        camera_component.expose()?;
        let camera_capture=products.runtime.capture_buffer_with_metadata("test-rgb",&camera,
            BufferLimits{max_entries:Some(4),max_bytes:Some(65536),target_duration:None},|_:&VideoFrame|128,
            |o|Ok(Some(CatalogProductMetadata{schema:"test.camera/v1".into(),source_sequence:o.sequence,
                value:serde_json::json!({"calibration":{"K":[100.,0.,320.,0.,100.,240.,0.,0.,1.],"D":[],
                    "distortion_model":"none","width":640,"height":480}})})))?;
        products.endpoint.export_product(&camera_capture.product())?;
        camera.publish(100,std::sync::Arc::new(VideoFrame{width:640,height:480,encoding:"jpeg".into(),bytes:vec![1,2,3].into()}))?;
        let routes = pair
            .b
            .listen_addresses()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let mut row =
            spatial_home::device_catalogs::fetch(&pair.a, pair.b.peer_id(), &routes).await;
        row["registries"] = spatial_home::product_registry::resolve(
            &pair.a,
            pair.b.peer_id(),
            &row,
            None,
            &definitions,
        )
        .await;
        assert_eq!(row["registries"]["status"], "ready", "{row}");
        let sensors=spatial_home::sensor_preview::camera_sensors(&std::collections::BTreeMap::from([(phone.clone(),row.clone())]));
        assert_eq!(sensors.len(),1);
        let client=auki_component_protocol::ComponentProtocolClient::new(pair.a.protocols());
        let frames=client.observations_exact::<VideoFrame>(pair.b.peer_id(),row["catalog_route"].as_str().unwrap().parse()?,auki_component_protocol::ObservationRequest{
            product:camera_capture.product().manifest.reference(),
            selection:auki_component_protocol::ObservationSelection::LatestExisting}).await?;
        let (catalog_tx, rx) =
            tokio::sync::watch::channel(std::collections::BTreeMap::from([(phone, row.clone())]));
        let stop = tokio_util::sync::CancellationToken::new();
        let held = spatial_home::frame_alignment::HeldTransforms::default();
        let subscriptions = held.serve(&pair.a, rx.clone(), &stop);
        let consumer = spatial_home::declared_depth::serve(&pair.a, rx, &scenes, &held, &stop);
        let check = async {
            let mut packed_published = false;
            tokio::time::timeout(std::time::Duration::from_secs(8), async {
                loop {
                    if let Some(publisher) = phone_depth.as_mut() && !packed_published
                        && products.endpoint.product_active_subscriptions(&spatial_home::mobile_declared::points_product_id(&session)) > 0 {
                        publisher.capture(100, &[[0.,-1.,-1.]], Some(&[[255,0,0]]))?;
                        packed_published = true;
                    }
                    if packed && !packed_published && products.endpoint.product_active_subscriptions("empty-until-subscribed") > 0 {
                        let (output, packet) = packed_send.as_ref().unwrap();
                        output.publish(100, std::sync::Arc::new(packet.clone()))?;
                        packed_published = true;
                    }
                    for publisher in &mut publishers {
                        publisher.tick()?;
                    }
                    let view = scenes.view("humanoid-hub-conference")?;
                    if view["integrated_observations"].as_u64().unwrap_or(0) > 0 {
                        assert!(!view["voxels"].as_array().unwrap().is_empty());
                        if extra_mount {
                            let (_,pose)=spatial_home::transform_chain::resolve(&client,&sensors[0],&row,
                                &(&point_frame).into(),&pc.clock,&pc.session_id,100,
                                &[(&spatial_home::conventions::registry(&home,&frame)?).into()],Some(&held)).await?;
                            assert_eq!(pose.translation,[3.,1.,0.]);
                        }
                        let ar=spatial_home::compact_ar::render(&client,&held,&sensors[0],&row,&frames.observations[0],&frames.producer,&scenes,&home,true).await?;
                        assert_eq!(ar["picking"]["frame"],frame);
                        assert_eq!(ar["ar"]["voxel_count"],0);
                        if packed {
                            let count = view["integrated_observations"].clone();
                            let discovered = catalog_tx.send_replace(std::collections::BTreeMap::new());
                            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                                while products.endpoint.product_active_subscriptions("empty-until-subscribed") != 0 {
                                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                                }
                            }).await?;
                            catalog_tx.send_replace(discovered);
                            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                                while products.endpoint.product_active_subscriptions("empty-until-subscribed") != 1 {
                                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                                }
                            }).await?;
                            // Rejoining requests new observations, never the last cached
                            // cloud. A genuinely new cloud without a pose at its capture
                            // time must not use the previous exact pose as a fallback.
                            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                            assert_eq!(scenes.view("humanoid-hub-conference")?["integrated_observations"], count);
                            let (output, packet) = packed_send.as_ref().unwrap();
                            output.publish(101, std::sync::Arc::new(packet.clone()))?;
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            assert_eq!(scenes.view("humanoid-hub-conference")?["integrated_observations"], count);
                        }
                        break Ok::<(), anyhow::Error>(());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            })
            .await?
        };
        let (_, _, checked) = tokio::join!(subscriptions, consumer, async {
            let result = check.await;
            stop.cancel();
            result
        });
        checked?;
        // Home's consumer shutdown must remove source demand, even with an idle cloud.
        if packed {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while products.endpoint.product_active_subscriptions("empty-until-subscribed") != 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }).await?;
        }
        Ok(())
    }
    .await;
    pair.shutdown().await?;
    result
}
