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
            frame: pc.from_frame.clone(),
            clock: pc.clock.clone(),
            session_id: session.clone(),
        };
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
            .in_registered_frame(auki_registry::FrameRegistryEntry::opengl(
                &phone,
                &pc.from_frame.id,
            )),
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
        let (_tx, rx) =
            tokio::sync::watch::channel(std::collections::BTreeMap::from([(phone, row)]));
        let stop = tokio_util::sync::CancellationToken::new();
        let consumer = spatial_home::declared_depth::serve(&pair.a, rx, &scenes, &stop);
        let check = async {
            tokio::time::timeout(std::time::Duration::from_secs(8), async {
                loop {
                    for publisher in &mut publishers {
                        publisher.tick()?;
                    }
                    let view = scenes.view("humanoid-hub-conference")?;
                    if view["integrated_observations"].as_u64().unwrap_or(0) > 0 {
                        assert!(!view["voxels"].as_array().unwrap().is_empty());
                        break Ok::<(), anyhow::Error>(());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            })
            .await?
        };
        let checked = tokio::select! {_ = consumer=>anyhow::bail!("consumer stopped"),r=check=>r};
        stop.cancel();
        checked
    }
    .await;
    pair.shutdown().await?;
    result
}
