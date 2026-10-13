#[allow(dead_code)]
mod support;
use anyhow::Result;
use spatial_home::{compact_pose::Contract, phone_alignment};
#[test]
fn tracking_loss_and_stale_samples_do_not_render_a_pose() -> Result<()> {
    let identity = auki_odometry::Pose {
        translation: [0.; 3],
        rotation_xyzw: [0., 0., 0., 1.],
    };
    let update = spatial_home::compact_pose::Update {
        tracking: auki_odometry::Tracking::Valid {
            pose: identity.clone(),
        },
    };
    assert!(matches!(
        phone_alignment::map_tracking(&update, &identity, std::time::Duration::ZERO)?,
        auki_odometry::Tracking::Valid { .. }
    ));
    assert!(matches!(
        phone_alignment::map_tracking(&update, &identity, std::time::Duration::from_secs(2))?,
        auki_odometry::Tracking::Lost
    ));
    let lost = spatial_home::compact_pose::Update {
        tracking: auki_odometry::Tracking::Lost,
    };
    assert!(matches!(
        phone_alignment::map_tracking(&lost, &identity, std::time::Duration::ZERO)?,
        auki_odometry::Tracking::Lost
    ));
    Ok(())
}
#[test]
fn held_chain_uses_exact_references_and_revocation() -> Result<()> {
    use spatial_home::compact_pose::{Lifetime, Update};
    let home = "local-scene:test-room:origin";
    let f = |id| auki_registry::FrameRegistryEntry::opengl("phone", id);
    let pose = Contract {
        from_frame: (&f("camera")).into(),
        to_frame: (&f("native")).into(),
        clock: spatial_home::model::clock_ref(&spatial_home::model::clock_entry(
            "phone", "session",
        )),
        session_id: "session".into(),
        lifetime: Lifetime::Sampled,
    };
    let mut held = Contract {
        from_frame: pose.to_frame.clone(),
        to_frame: (&spatial_home::conventions::registry("home", home)?).into(),
        lifetime: Lifetime::Held,
        ..pose.clone()
    };
    phone_alignment::validate_chain(&held, &pose, "phone", "home", home)?;
    held.to_frame.hash = "wrong".into();
    assert!(phone_alignment::validate_chain(&held, &pose, "phone", "home", home).is_err());
    held.to_frame = (&spatial_home::conventions::registry("home", home)?).into();
    held.clock.id = "wrong".into();
    assert!(phone_alignment::validate_chain(&held, &pose, "phone", "home", home).is_err());
    let valid = Update {
        tracking: auki_odometry::Tracking::Valid {
            pose: auki_odometry::Pose {
                translation: [4., 0., 2.],
                rotation_xyzw: [0., 0., 0., 1.],
            },
        },
    };
    let lost = Update {
        tracking: auki_odometry::Tracking::Lost,
    };
    assert!(phone_alignment::held_at(&[(10, valid.clone())], 9).is_err());
    assert_eq!(
        phone_alignment::held_at(&[(10, valid.clone())], u64::MAX)?.1,
        10
    );
    let history = [(10, valid.clone()), (20, lost), (30, valid)];
    assert_eq!(phone_alignment::held_at(&history, 19)?.1, 10);
    assert!(phone_alignment::held_at(&history, 25).is_err());
    assert_eq!(phone_alignment::held_at(&history, 30)?.1, 30);
    Ok(())
}

#[test]
fn sampled_depth_pose_never_extrapolates_or_crosses_loss_or_gaps() -> Result<()> {
    use spatial_home::{compact_pose::Update, declared_depth::sampled_at};
    let valid = |x| Update {
        tracking: auki_odometry::Tracking::Valid {
            pose: auki_odometry::Pose {
                translation: [x, 0., 0.],
                rotation_xyzw: [0., 0., 0., 1.],
            },
        },
    };
    let samples = [(1, 100, valid(0.)), (2, 200, valid(2.))];
    assert_eq!(sampled_at(&samples, 150)?.translation, [1., 0., 0.]);
    assert!(sampled_at(&samples, 99).is_err());
    assert!(sampled_at(&samples, 201).is_err());
    assert!(sampled_at(&[(1, 100, valid(0.)), (3, 200, valid(2.))], 150).is_err());
    assert!(sampled_at(&[(1, 100, valid(0.)), (2, 600_000_100, valid(2.))], 150).is_err());
    assert!(
        sampled_at(
            &[
                (1, 100, valid(0.)),
                (
                    2,
                    200,
                    Update {
                        tracking: auki_odometry::Tracking::Lost
                    }
                )
            ],
            150
        )
        .is_err()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn home_fetches_exact_registry_definitions_before_inspection() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let home_definitions = spatial_home::product_registry::Definitions::default();
        let home_frame = spatial_home::conventions::registry(
            &pair.a.peer_id().to_string(),
            "local-scene:test-room:portal-ABCDEFGHIJK",
        )?;
        spatial_home::product_registry::frame(&home_definitions, &home_frame)?;
        let home_registry = spatial_home::product_registry::mount(
            &pair.a,
            auki_components::ComponentRuntime::new(pair.a.peer_id().to_string()),
            home_definitions.clone(),
        )?;
        let home_route = pair.a.listen_addresses()[0].clone();
        let fetched = auki_protocols::registry::RegistryClient::new(pair.b.protocols())
            .fetch_frame_exact(
                pair.a.peer_id(),
                home_route,
                &home_frame.frame_id,
                &home_frame.hash(),
            )
            .await?;
        assert_eq!(fetched, home_frame);
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let session = uuid::Uuid::new_v4().to_string();
        let sample = spatial_home::mobile_pose::Sample {
            session: session.clone(),
            timestamp_ns: 1,
            camera_configuration_id: None,
            pose: auki_scenegraph::RigidTransform::identity(
                format!("phone-camera:{session}"),
                format!("phone-ar:{session}"),
            ),
        };
        let mut publisher = spatial_home::mobile_odometry::Publisher::new(
            &products,
            &pair.a.peer_id().to_string(),
            &sample,
        )?;
        publisher.publish(&pair.a.peer_id().to_string(), &sample)?;
        let routes = pair
            .b
            .listen_addresses()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let mut row =
            spatial_home::device_catalogs::fetch(&pair.a, pair.b.peer_id(), &routes).await;
        let registry = spatial_home::product_registry::resolve(
            &pair.a,
            pair.b.peer_id(),
            &row,
            None,
            &Default::default(),
        )
        .await;
        assert_eq!(registry["status"], "ready", "{registry}");
        let c: Contract =
            serde_json::from_value(row["catalog"]["products"][0]["metadata"]["value"].clone())?;
        let (resolved, _) = spatial_home::product_registry::resolved_contract(&registry, &c)?;
        assert_eq!(resolved.to_frame.frame_id, format!("phone-ar:{session}"));
        row["catalog"]["products"][0]["metadata"]["value"]["from_frame"]["hash"] =
            serde_json::json!("a".repeat(32));
        let broken = spatial_home::product_registry::resolve(
            &pair.a,
            pair.b.peer_id(),
            &row,
            Some(&registry),
            &Default::default(),
        )
        .await;
        assert_eq!(broken["status"], "incomplete");
        let changed: Contract =
            serde_json::from_value(row["catalog"]["products"][0]["metadata"]["value"].clone())?;
        assert!(spatial_home::product_registry::resolved_contract(&broken, &changed).is_err());
        publisher.close()?;
        products.close().await?;
        home_registry.close().await?;
        Ok(())
    }
    .await;
    result.and(pair.shutdown().await)
}
