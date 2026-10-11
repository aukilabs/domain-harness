mod support;
use anyhow::Result;
use auki_scenegraph::RigidTransform;
use spatial_home::{
    local_mapper::{compose, inverse},
    mobile_depth::{Alignment, Inbox, Publisher, Sample},
    model::{FRAME, Store},
    network::{HomeConnection, ScenePublisher},
};
fn from_rigid(p: &RigidTransform) -> auki_odometry::Pose {
    let [w, x, y, z] = p.rotation_wxyz;
    auki_odometry::Pose {
        translation: p.translation,
        rotation_xyzw: [x, y, z, w],
    }
}
fn rigid(p: &auki_odometry::Pose, from: &str, to: &str) -> RigidTransform {
    let [x, y, z, w] = p.rotation_xyzw;
    RigidTransform {
        from_frame_id: from.into(),
        to_frame_id: to.into(),
        translation: p.translation,
        rotation_wxyz: [w, x, y, z],
    }
}
fn sample(canonical: &auki_scenegraph::MapSnapshot, session: &str) -> Result<Sample> {
    let portal = &canonical.scenegraph.anchors["UCOF1J40N7O"];
    let session_frame = format!("phone-ar:{session}");
    let home_from_session = auki_odometry::Pose {
        translation: [4., 0., 0.],
        rotation_xyzw: [
            0.,
            (std::f64::consts::FRAC_PI_4).sin(),
            0.,
            (std::f64::consts::FRAC_PI_4).cos(),
        ],
    };
    let portal_in_session = compose(
        &inverse(&home_from_session)?,
        &from_rigid(&portal.pose_in_map),
    )?;
    Ok(Sample {
        session: session.into(),
        timestamp_ns: 200,
        camera_to_session: rigid(
            &auki_odometry::Pose {
                translation: [0., 1., 0.],
                rotation_xyzw: [0., 0., 0., 1.],
            },
            &format!("phone-camera:{session}"),
            &session_frame,
        ),
        alignment: Alignment {
            timestamp_ns: 100,
            portal_id: portal.anchor_id.clone(),
            portal_to_session: rigid(
                &portal_in_session,
                &portal.pose_in_map.from_frame_id,
                &session_frame,
            ),
            session_to_home: rigid(&home_from_session, &session_frame, FRAME),
        },
        points: vec![[0., 0., -1.]],
    })
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_local_depth_reaches_home_and_is_transformed_once() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let store = Store::open_canonical(dir.path().join("home.json"))?;
        let canonical = store.built.portals;
        let inbox = std::sync::Arc::new(std::sync::Mutex::new(Inbox::default()));
        let server = ScenePublisher::mount(&pair.a)?;
        server.enable_phone_depth(inbox.clone(), canonical.clone())?;
        let session = uuid::Uuid::new_v4().to_string();
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let publisher = Publisher::new(&products, &session)?;
        let packet = sample(&canonical, &session)?;
        let envelope = publisher.publish(packet.clone())?;
        assert_eq!(
            envelope.sample.points,
            vec![[0., 0., -1.]],
            "points remain phone-local"
        );
        assert_ne!(envelope.sample.camera_to_session.to_frame_id, FRAME);
        let home = HomeConnection::direct(&pair.a);
        assert!(spatial_home::mobile_depth::send(&pair.b, &home, &envelope).await?);
        assert!(
            !spatial_home::mobile_depth::send(&pair.b, &home, &envelope).await?,
            "duplicate captures ignored"
        );
        let mut map = spatial_home::collaborative_mapper::CollaborativeMap::new(
            &pair.a.peer_id().to_string(),
            &uuid::Uuid::new_v4().to_string(),
            &canonical,
        )?;
        for frame in inbox.lock().unwrap().take() {
            assert!(map.integrate_phone(frame)?);
        }
        let scene = map.scene()?.unwrap();
        scene.validate(&pair.a.peer_id().to_string())?;
        let render = spatial_home::mobile_geometry::render(&scene)?;
        let cells = render["voxels"].as_array().unwrap();
        assert!(
            !cells.is_empty(),
            "phone-only input creates Home live geometry"
        );
        assert!(
            cells.iter().any(|v| {
                let c = v["center"].as_array().unwrap();
                (c[0].as_f64().unwrap() - 3.).abs() < 0.101
                    && (c[1].as_f64().unwrap() - 1.).abs() < 0.101
                    && c[2].as_f64().unwrap().abs() < 0.101
            }),
            "rotated session origin produces expected Home hit [3,1,0]"
        );
        assert_eq!(
            scene.registries["collaborative_mapping"]["contributors"][pair.b.peer_id().to_string()],
            1
        );
        assert_eq!(
            scene.voxels.definition.sources[0].sensor_product,
            envelope.source
        );
        let mut invalid = envelope.clone();
        invalid.sample.alignment.session_to_home.translation[0] += 0.5;
        assert!(
            spatial_home::mobile_depth::validate(
                &pair.b.peer_id().to_string(),
                &invalid,
                &canonical
            )
            .is_err()
        );
        let mut invalid = envelope.clone();
        invalid.sample.camera_to_session.to_frame_id = "another-session".into();
        assert!(
            spatial_home::mobile_depth::validate(
                &pair.b.peer_id().to_string(),
                &invalid,
                &canonical
            )
            .is_err()
        );
        let mut invalid = envelope.clone();
        invalid.sample.timestamp_ns = 50;
        assert!(
            spatial_home::mobile_depth::validate(
                &pair.b.peer_id().to_string(),
                &invalid,
                &canonical
            )
            .is_err()
        );
        let mut invalid = envelope.clone();
        invalid.sample.points = vec![[0., 0., -1.]; 2049];
        assert!(
            spatial_home::mobile_depth::validate(
                &pair.b.peer_id().to_string(),
                &invalid,
                &canonical
            )
            .is_err()
        );
        assert!(
            spatial_home::mobile_depth::validate("another-peer", &envelope, &canonical).is_err()
        );
        let mut newer = envelope.clone();
        newer.sample.timestamp_ns = 300;
        newer.sequence += 1;
        inbox
            .lock()
            .unwrap()
            .accept(&pair.b.peer_id().to_string(), newer, &canonical)?;
        let mut newest = envelope;
        newest.sample.timestamp_ns = 400;
        newest.sequence += 2;
        inbox
            .lock()
            .unwrap()
            .accept(&pair.b.peer_id().to_string(), newest, &canonical)?;
        let pending = inbox.lock().unwrap().take();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].sample.timestamp_ns, 400,
            "no obsolete frame backlog"
        );
        let previous_source = pending[0].source.clone();
        publisher.close().await?;
        products.close().await?;
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let resumed = Publisher::new(&products, &session)?;
        let mut later = packet; later.timestamp_ns = 500;
        let resumed_envelope = resumed.publish(later)?;
        assert_ne!(resumed_envelope.source.manifest_hash, previous_source.manifest_hash,
                   "a reconnected publisher has a fresh source sequence identity while AR space stays unchanged");
        assert!(spatial_home::mobile_depth::send(&pair.b, &home, &resumed_envelope).await?);
        resumed.close().await?;
        products.close().await?;
        server.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_phone_catalog_serves_both_products_across_ar_resets() -> Result<()> {
    use spatial_home::{mobile_local, mobile_products::MobileProducts};
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let canonical = Store::open_canonical(dir.path().join("home.json"))?
            .built
            .portals;
        let inbox = std::sync::Arc::new(std::sync::Mutex::new(Inbox::default()));
        let server = ScenePublisher::mount(&pair.a)?;
        server.enable_phone_depth(inbox.clone(), canonical.clone())?;
        mobile_local::mount_receiver(&server.runtime, &server.endpoint)?;
        let home = HomeConnection::direct(&pair.a);
        let routes = pair
            .b
            .listen_addresses()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        for local_first in [true, false] {
            let products = MobileProducts::mount(&pair.b)?;
            let session = uuid::Uuid::new_v4().to_string();
            let (local, mut depth) = if local_first {
                let local = mobile_local::Publisher::new(&products)?;
                (local, Publisher::new(&products, &session)?)
            } else {
                let depth = Publisher::new(&products, &session)?;
                (mobile_local::Publisher::new(&products)?, depth)
            };
            let envelope = local.publish(mobile_local::Snapshot {
                session: session.clone(),
                timestamp_ns: 100,
                cells: vec![[1, 2, 3]],
            })?;
            mobile_local::send(&pair.b, &home, &envelope).await?;
            let envelope = depth.publish(sample(&canonical, &session)?)?;
            assert!(spatial_home::mobile_depth::send(&pair.b, &home, &envelope).await?);
            let mut map = spatial_home::collaborative_mapper::CollaborativeMap::new(
                &pair.a.peer_id().to_string(),
                &uuid::Uuid::new_v4().to_string(),
                &canonical,
            )?;
            for frame in inbox.lock().unwrap().take() {
                assert!(map.integrate_phone(frame)?);
            }
            assert!(
                !spatial_home::mobile_geometry::render(&map.scene()?.unwrap())?["voxels"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            let before =
                spatial_home::device_catalogs::fetch(&pair.a, pair.b.peer_id(), &routes).await;
            assert_eq!(before["catalog_status"], "ready");
            assert_eq!(before["catalog"]["components"].as_array().unwrap().len(), 2);
            assert_eq!(before["catalog"]["products"].as_array().unwrap().len(), 2);
            let mut previous = session;
            for _ in 0..3 {
                let next = uuid::Uuid::new_v4().to_string();
                depth.reconfigure(&next, 300)?;
                assert!(
                    spatial_home::mobile_depth::send(
                        &pair.b,
                        &home,
                        &depth.publish(sample(&canonical, &next)?)?
                    )
                    .await?
                );
                let catalog =
                    spatial_home::device_catalogs::fetch(&pair.a, pair.b.peer_id(), &routes).await;
                assert_eq!(
                    catalog["catalog"]["components"].as_array().unwrap().len(),
                    2,
                    "stable components, not one per reset"
                );
                let entries = catalog["catalog"]["products"].as_array().unwrap();
                assert_eq!(entries.len(), 2, "retired depth Products are removed");
                assert!(
                    entries
                        .iter()
                        .any(|e| e["manifest"]["product_id"] == format!("phone-depth:{next}"))
                );
                assert!(
                    !entries
                        .iter()
                        .any(|e| e["manifest"]["product_id"] == format!("phone-depth:{previous}"))
                );
                previous = next;
            }
            depth.close().await?;
            let catalog =
                spatial_home::device_catalogs::fetch(&pair.a, pair.b.peer_id(), &routes).await;
            assert_eq!(
                catalog["catalog_status"], "ready",
                "closing depth must not close shared protocols"
            );
            assert_eq!(catalog["catalog"]["products"].as_array().unwrap().len(), 1);
            let envelope = local.publish(mobile_local::Snapshot {
                session: previous,
                timestamp_ns: 400,
                cells: vec![[4, 5, 6]],
            })?;
            mobile_local::send(&pair.b, &home, &envelope).await?;
            local.close().await?;
            products.close().await?;
        }
        server.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(pair.shutdown().await)
}
