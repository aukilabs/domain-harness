//! A colored phone-scanned map must keep accepting captures well past the
//! former 4 MiB snapshot bound, and a phone must still receive the scene.
mod support;
use anyhow::Result;
use spatial_home::{
    local_scenes::LocalScenes,
    mobile_depth::{Alignment, Sample},
    network::{HomeConnection, ScenePublisher},
};
use std::{
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

fn from_rigid(p: &auki_scenegraph::RigidTransform) -> auki_odometry::Pose {
    let [w, x, y, z] = p.rotation_wxyz;
    auki_odometry::Pose {
        translation: p.translation,
        rotation_xyzw: [x, y, z, w],
    }
}
fn rigid(p: &auki_odometry::Pose, from: &str, to: &str) -> auki_scenegraph::RigidTransform {
    let [x, y, z, w] = p.rotation_xyzw;
    auki_scenegraph::RigidTransform {
        from_frame_id: from.into(),
        to_frame_id: to.into(),
        translation: p.translation,
        rotation_wxyz: [w, x, y, z],
    }
}
async fn read(peer: &auki_sdk::AukiPeer, home: &HomeConnection) -> Result<serde_json::Value> {
    let mut value = None;
    tokio::time::timeout(
        Duration::from_secs(20),
        spatial_home::mobile::receive_portals(
            peer,
            home,
            &CancellationToken::new(),
            Some(Duration::from_millis(50)),
            |v| value = Some(v),
        ),
    )
    .await??;
    Ok(value.expect("retained scene"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "about a minute: one capture per one-second Home tick"]
async fn a_large_colored_map_keeps_accepting_captures() -> Result<()> {
    use spatial_home::local_mapper::{compose, identity, inverse};
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let scenes = Arc::new(LocalScenes::load(dir.path())?);
        let store = spatial_home::model::Store::open_canonical(dir.path().join("xmart.journal"))?;
        let mut server = ScenePublisher::mount(&pair.a)?;
        server.publish(&store.built)?;
        let mut publishers = spatial_home::local_mobile::Publisher::mount_all(
            &server.runtime,
            &server.endpoint,
            scenes.clone(),
            server.session_clock.clone(),
        )?;
        let home = scenes.connection("humanoid-hub-conference", HomeConnection::direct(&pair.a))?;
        let reference = scenes.snapshot("humanoid-hub-conference")?;
        let portal = &reference.scenegraph.anchors["HNY3JGNLRAE"];
        let session = uuid::Uuid::new_v4().to_string();
        let session_frame = format!("phone-ar:{session}");
        let camera = format!("phone-camera:{session}");
        let home_from_session = identity();
        let session_from_portal = compose(
            &inverse(&home_from_session)?,
            &from_rigid(&portal.pose_in_map),
        )?;
        let alignment = Alignment {
            timestamp_ns: 100,
            portal_id: portal.anchor_id.clone(),
            portal_to_session: rigid(
                &session_from_portal,
                &portal.pose_in_map.from_frame_id,
                &session_frame,
            ),
            session_to_home: rigid(&home_from_session, &session_frame, &home.frame),
        };
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let phone = spatial_home::mobile_depth::Publisher::new(&products, &session)?;
        let captures: u64 = std::env::var("LARGE_MAP_CAPTURES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(45);
        for k in 0..captures {
            // Walk the camera across a 9 m by 9 m area, sweeping a fan of colored
            // surface points 1 to 2 m ahead from each stop.
            let (row, column) = ((k / 9) as f64, (k % 9) as f64);
            let mut points = Vec::new();
            let mut colors = Vec::new();
            for i in 0..32 {
                for j in 0..32 {
                    let (a, b) = ((i as f64 / 31. - 0.5) * 1.6, (j as f64 / 31. - 0.5) * 1.2);
                    let range = 1. + ((i * 7 + j * 13 + k as usize * 3) % 10) as f64 * 0.1;
                    points.push([range * a.sin(), range * b.sin(), -range * a.cos() * b.cos()]);
                    colors.push([(i * 8) as u8, (j * 8) as u8, (k * 5) as u8]);
                }
            }
            let sample = Sample {
                session: session.clone(),
                timestamp_ns: 200 + k * 10,
                camera_to_session: rigid(
                    &auki_odometry::Pose {
                        translation: [column - 4., 1.2, row - 2.],
                        ..identity()
                    },
                    &camera,
                    &session_frame,
                ),
                alignment: alignment.clone(),
                points,
                colors: Some(colors),
            };
            assert!(
                spatial_home::mobile_depth::send(&pair.b, &home, &phone.publish(sample)?).await?,
                "capture {k} accepted by the inbox"
            );
            tokio::time::sleep(Duration::from_millis(1050)).await;
            for publisher in &mut publishers {
                publisher.tick()?;
            }
        }
        let scene = read(&pair.b, &home).await?;
        let voxels = scene["render"]["voxels"].as_array().unwrap().len();
        let colored = scene["render"]["voxels"].as_array().unwrap().iter().filter(|v| v["linear_rgb"].is_array()).count();
        let integrated = scene["render"]["integrated_observations"].as_u64().unwrap();
        let bytes = serde_json::to_vec(&scene)?.len();
        let status = &scene["render"]["local_mapping"]["runners"][pair.b.peer_id().to_string()];
        eprintln!("captures {captures}: integrated {integrated}, voxels {voxels} ({colored} colored), phone scene {} KiB, status {status}", bytes / 1024);
        assert_eq!(integrated, captures, "every capture was integrated: {status}");
        assert_ne!(status["status"], "rejected");
        drop(publishers);
        server.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}
