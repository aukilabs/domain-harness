mod support;
use spatial_home::{
    mobile_pose::{Presence, Sample, Update},
    model::FRAME,
};
use std::time::{Duration, Instant};
fn sample(session: &str, timestamp_ns: u64) -> Sample {
    Sample {
        session: session.into(),
        timestamp_ns,
        camera_configuration_id:None,
        pose: auki_scenegraph::RigidTransform {
            from_frame_id: format!("phone-camera:{session}"),
            to_frame_id: FRAME.into(),
            translation: [1., 1.5, 2.],
            rotation_wxyz: [1., 0., 0., 0.],
        },
    }
}
#[test]
fn pose_identity_frames_and_expiry() -> anyhow::Result<()> {
    let mut cache = Presence::default();
    let now = Instant::now();
    let session = uuid::Uuid::new_v4().to_string();
    let update = Update {
        sample: Some(sample(&session, 100)),
        status: None,
    };
    cache.accept("phone", update.clone(), now)?;
    assert_eq!(cache.snapshot(now)["poses"][0]["peer_id"], "phone");
    assert_eq!(
        cache.snapshot(now)["poses"][0]["name"],
        spatial_home::mobile_pose::display_name("phone")
    );
    cache.accept("phone", update, now + Duration::from_secs(2))?;
    assert_eq!(
        cache.snapshot(now + Duration::from_secs(3))["poses"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "duplicate capture must not refresh freshness"
    );
    let mut invalid = sample(&session, 200);
    invalid.pose.to_frame_id = "AR-local".into();
    assert!(
        cache
            .accept(
                "phone",
                Update {
                    sample: Some(invalid),
                    status: None,
                },
                now
            )
            .is_err()
    );
    cache.accept(
        "phone",
        Update {
            sample: Some(sample(&session, 300)),
            status: None,
        },
        now,
    )?;
    cache.accept(
        "phone",
        Update {
            sample: None,
            status: None,
        },
        now,
    )?;
    assert_eq!(cache.snapshot(now)["poses"].as_array().unwrap().len(), 0);
    Ok(())
}
#[test]
fn asleep_phone_keeps_its_last_pose_until_the_heartbeat_lapses() -> anyhow::Result<()> {
    let mut cache = Presence::default();
    let now = Instant::now();
    let session = uuid::Uuid::new_v4().to_string();
    let asleep = || Update {
        sample: None,
        status: Some("asleep".into()),
    };
    // Sleeping before any pose is known lists nothing.
    cache.accept("phone", asleep(), now)?;
    assert_eq!(cache.snapshot(now)["poses"].as_array().unwrap().len(), 0);
    let live = Update {
        sample: Some(sample(&session, 100)),
        status: None,
    };
    cache.accept("phone", live.clone(), now)?;
    assert_eq!(cache.snapshot(now)["poses"][0]["state"], "live");
    cache.accept("phone", asleep(), now)?;
    let later = now + Duration::from_secs(60);
    let listed = cache.snapshot(later);
    assert_eq!(listed["poses"][0]["state"], "asleep");
    assert_eq!(
        listed["poses"][0]["pose"]["translation"],
        serde_json::json!(sample(&session, 100).pose.translation)
    );
    assert_eq!(
        cache.snapshot(now + spatial_home::mobile_pose::ASLEEP_TTL)["poses"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // A live update must not carry a status, and unknown statuses are rejected.
    let mut wrong = live.clone();
    wrong.status = Some("asleep".into());
    assert!(cache.accept("phone", wrong, later).is_err());
    assert!(
        cache
            .accept(
                "phone",
                Update {
                    sample: None,
                    status: Some("dancing".into())
                },
                later
            )
            .is_err()
    );
    // Old wire format still decodes, and live updates still encode without the field.
    let old: Update = serde_json::from_str(r#"{"sample":null}"#)?;
    assert!(old.status.is_none());
    assert!(!serde_json::to_string(&live)?.contains("status"));
    // Waking with a fresh pose makes it live again.
    cache.accept(
        "phone",
        Update {
            sample: Some(sample(&session, 200)),
            status: None,
        },
        later,
    )?;
    assert_eq!(cache.snapshot(later)["poses"][0]["state"], "live");
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn calibrated_pose_reaches_home_over_p2p() -> anyhow::Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let store = std::sync::Arc::new(std::sync::Mutex::new(spatial_home::model::Store::open(
            dir.path().join("home.json"),
        )?));
        let publisher = spatial_home::network::ScenePublisher::mount(&pair.a)?;
        let changed = std::sync::Arc::new(tokio::sync::Notify::new());
        publisher.enable_phone_presence(store.clone(), changed)?;
        let home = spatial_home::network::HomeConnection::direct(&pair.a);
        let update = Update {
            sample: Some(sample(&uuid::Uuid::new_v4().to_string(), 100)),
            status: None,
        };
        let mut sender = spatial_home::mobile_pose::PresenceSender::default();
        sender.send(&pair.b, &home, spatial_home::mobile_pose::COMPONENT, &update).await?;
        let initial_lookups = sender.catalog_requests();
        assert!(initial_lookups > 0);
        // A different component must not reuse the cached presence receiver.
        assert!(sender.send(&pair.b, &home, "missing-presence", &update).await.is_err());
        let mut fresh = update.clone();
        fresh.sample.as_mut().unwrap().timestamp_ns = 200;
        sender.send(&pair.b, &home, spatial_home::mobile_pose::COMPONENT, &fresh).await?;
        let rediscovered = sender.catalog_requests();
        assert!(rediscovered > initial_lookups);
        fresh.sample.as_mut().unwrap().timestamp_ns = 300;
        sender.send(&pair.b, &home, spatial_home::mobile_pose::COMPONENT, &fresh).await?;
        assert_eq!(sender.catalog_requests(), rediscovered, "steady-state poses must not fetch Catalog");
        let mut invalid = fresh.clone();
        invalid.sample.as_mut().unwrap().pose.to_frame_id = "wrong-home".into();
        assert!(sender.send(&pair.b, &home, spatial_home::mobile_pose::COMPONENT, &invalid).await.is_err());
        fresh.sample.as_mut().unwrap().timestamp_ns = 400;
        sender.send(&pair.b, &home, spatial_home::mobile_pose::COMPONENT, &fresh).await?;
        assert!(sender.catalog_requests() > rediscovered, "failed sends must invalidate discovery");
        let registry = store.lock().unwrap().built.registries["mobile_presence"].clone();
        assert_eq!(
            registry["poses"][0]["peer_id"],
            pair.b.peer_id().to_string()
        );
        let mut view = serde_json::json!({"notes":[]});
        spatial_home::mobile_pose::append_labels(&mut view, &registry);
        assert_eq!(view["notes"][0]["text"], registry["poses"][0]["name"]);
        assert_eq!(
            view["notes"][0]["position"],
            serde_json::json!([1., 1.5, 2.])
        );
        sender.send(
            &pair.b,
            &home,
            spatial_home::mobile_pose::COMPONENT,
            &Update {
                sample: None,
                status: None,
            },
        )
        .await?;
        assert!(
            store.lock().unwrap().built.registries["mobile_presence"]["poses"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        publisher.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[test]
fn a_phone_names_itself_without_a_restart() -> anyhow::Result<()> {
    use spatial_home::mobile_pose::{display_name, set_name};
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("names.json");
    // SAFETY: only the names functions read this variable.
    unsafe { std::env::set_var("SPATIAL_PHONE_NAMES", &path) };
    let peer = format!("peer-{}", uuid::Uuid::new_v4());
    assert!(display_name(&peer).starts_with("phone "));
    set_name(&peer, "  spaceship ")?;
    assert_eq!(display_name(&peer), "spaceship", "applies at once, trimmed");
    let saved: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(&path)?)?;
    assert_eq!(saved[&peer], "spaceship", "saved for the next start");
    set_name(&peer, "tablet")?;
    assert_eq!(display_name(&peer), "tablet");
    for bad in ["", "   ", "a\nb", &"x".repeat(25)] {
        assert!(set_name(&peer, bad).is_err(), "rejected: {bad:?}");
    }
    assert!(set_name("", "x").is_err());
    assert!(
        set_name("*", "everyone").is_err(),
        "the wildcard is not a phone"
    );
    assert_eq!(display_name(&peer), "tablet");
    Ok(())
}
