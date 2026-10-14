#[allow(dead_code)]
mod support;
use anyhow::Result;
use serde_json::json;
use spatial_home::{device_catalogs::{Catalogs, fetch}, peer_locations, sensor_preview};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

/// The chat tool reuses the viewer's authenticated SDK consumer. An unrelated
/// offline peer cannot delay it, and capture nanoseconds survive intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn targeted_peer_pose_preserves_capture_provenance_over_authenticated_sdk() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let dir = tempfile::tempdir()?;
        let homes = support::seeded_local_scenes(dir.path())?;
        let home_frame = homes.snapshots["humanoid-hub-conference"].scenegraph.map.frame.id.clone();
        let home_peer = pair.a.peer_id().to_string();
        let owner = pair.b.peer_id().to_string();
        let session = uuid::Uuid::new_v4().to_string();
        let capture_time = 1_700_000_000_000_000_003_u64;
        let mut sample = spatial_home::mobile_pose::Sample {
            session: session.clone(), timestamp_ns: capture_time, camera_configuration_id: None,
            pose: auki_scenegraph::RigidTransform {
                from_frame_id: format!("phone-camera:{session}"), to_frame_id: format!("phone-ar:{session}"),
                translation: [1., 1.5, 2.], rotation_wxyz: [1., 0., 0., 0.],
            },
        };
        let mut pose = spatial_home::mobile_odometry::Publisher::new(&products, &home_peer, &sample)?;
        pose.publish(&home_peer, &sample)?;
        let mut declared = spatial_home::mobile_declared::Publisher::new(&products, &session, &home_peer, &home_frame)?;
        declared.align(capture_time - 1, &auki_scenegraph::RigidTransform {
            from_frame_id: sample.pose.to_frame_id.clone(), to_frame_id: home_frame.clone(),
            translation: [2., 0., 3.], rotation_wxyz: [1., 0., 0., 0.],
        })?;
        let definitions = spatial_home::product_registry::Definitions::default();
        spatial_home::product_registry::frame(&definitions, &spatial_home::conventions::registry(&home_peer, &home_frame)?)?;
        let routes = pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
        let mut row = fetch(&pair.a, pair.b.peer_id(), &routes).await;
        row["peer_id"] = json!(owner);
        row["registries"] = spatial_home::product_registry::resolve(&pair.a, pair.b.peer_id(), &row, None, &definitions).await;
        let reference = row["catalog"]["products"].as_array().unwrap().iter()
            .find(|p|p["manifest"]["product_id"]==spatial_home::mobile_odometry::product_id(&session)).unwrap().clone();
        let expected_contract = spatial_home::mobile_odometry::contract(&owner, &home_peer, &sample);
        let catalogs = Catalogs::from([(owner.clone(), row.clone())]);
        let network = json!({"peers":[{"peer_id":"unrelated-offline","catalog":null},row]});
        let (_catalog_tx, catalog_rx) = watch::channel(catalogs);
        let (tx, rx) = mpsc::channel(4);
        let (_live_tx, live) = watch::channel(spatial_home::scene_mirror::RemoteScene::default());
        let frames = spatial_home::ar_preview::Frames::default();
        let stop = CancellationToken::new();
        let held = spatial_home::frame_alignment::HeldTransforms::default();
        let subscriptions = held.serve(&pair.a, catalog_rx.clone(), &stop);
        let serve = sensor_preview::serve(&pair.a, catalog_rx, rx, live, &frames, &homes, &held, &stop);
        let exercise = async {
            let query = json!({"peer_id":owner});
            let mut last = String::new();
            let seen = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    // The viewer's sampled subscription is NewOnly. Publish a
                    // subsequent real observation while waiting on that exact peer.
                    let lookup = peer_locations::get(&tx, &network, query.clone(), &home_frame);
                    let publish = async {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        sample.timestamp_ns += 1;
                        pose.publish(&home_peer, &sample)
                    };
                    let (result, sent) = tokio::join!(lookup, publish);
                    sent?;
                    match result { Ok(value) => break Ok::<_, anyhow::Error>(value), Err(e) => last = e.to_string() }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }).await.map_err(|_|anyhow::anyhow!("Target pose was not resolved: {last}"))??;
            let location = &seen["location"];
            assert_eq!(seen["peer_id"], owner);
            assert_eq!(location["position"], json!([3., 1.5, 5.]));
            assert_eq!(location["rotation_xyzw"], json!([0., 0., 0., 1.]));
            assert_eq!(location["frame"], home_frame);
            assert_eq!(location["clock"], json!(expected_contract.clock));
            let received_sequence = location["source"]["sequence"].as_str().unwrap().parse::<u64>()?;
            assert_eq!(location["capture_timestamp_ns"].as_str().unwrap().parse::<u64>()?, capture_time + received_sequence);
            assert_eq!(location["alignment_timestamp_ns"], (capture_time - 1).to_string());
            assert_eq!(location["source"]["output"], reference["manifest"]["producer"]);
            assert_eq!(location["source"]["product"]["manifest_hash"], reference["manifest_hash"]);
            assert_eq!(location["source"]["contract"]["clock"], json!(expected_contract.clock));
            assert_eq!(location["source"]["contract"]["session_id"], session);
            assert_eq!(location["source"]["frame_definitions"]["from_frame"], json!(expected_contract.from_frame));
            assert_eq!(location["source"]["frame_definitions"]["to_frame"], json!(expected_contract.to_frame));
            assert!(received_sequence > 0);
            assert_eq!(location["freshness"]["basis"], "arrival");
            assert!(location["received_age_ms"].as_u64().unwrap() < 2000);
            // The inspector's history is deliberately not copied into chat.
            assert!(location.get("history").is_none());
            tokio::time::sleep(Duration::from_millis(2100)).await;
            assert!(peer_locations::get(&tx, &network, query.clone(), &home_frame).await.is_err(), "a retained stale sample is never a current location");
            pose.lost()?;
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let error = peer_locations::get(&tx, &network, query.clone(), &home_frame).await.unwrap_err();
                    if error.to_string() == "Tracking is not valid" {break;}
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }).await?;
            Ok::<_, anyhow::Error>(())
        };
        let (_, _, result) = tokio::join!(serve, subscriptions, async {let result=exercise.await;stop.cancel();result});
        pose.close()?;
        declared.close()?;
        products.close().await?;
        result
    }.await;
    result.and(pair.shutdown().await)
}
