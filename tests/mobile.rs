mod support;
use anyhow::Result;
use spatial_home::{
    mobile::receive_portals,
    model::{Event, Store, fixtures},
    network::{HomeConnection, ScenePublisher},
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_receives_initial_and_live_portals_and_awaits_cancellation() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(directory.path().join("home.json"))?;
        let mut publisher = ScenePublisher::mount(&pair.a)?;
        publisher.publish(&store.built)?;
        let home = HomeConnection::direct(&pair.a);
        let stop = CancellationToken::new();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let consume = receive_portals(&pair.b, &home, &stop, Duration::from_secs(30), |portals| {
            sender.send(portals).unwrap();
        });
        let check = async {
            let portals = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
                .await?
                .unwrap();
            assert_eq!(
                portals["scenegraph"]["anchors"].as_object().unwrap().len(),
                14
            );
            assert!(
                portals.get("voxels").is_none(),
                "raw voxel checkpoint must not cross the mobile ABI"
            );
            assert_eq!(portals["render"]["frame"], spatial_home::model::FRAME);
            assert!(portals["render"]["notes"].is_array());
            store.apply(Event::Contribution(serde_json::from_value(
                fixtures()?["contributions"][0].clone(),
            )?))?;
            publisher.publish(&store.built)?;
            let updated = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
                .await?
                .unwrap();
            assert!(!updated["render"]["voxels"].as_array().unwrap().is_empty());
            stop.cancel();
            Ok::<_, anyhow::Error>(())
        };
        let (consumed, checked) = tokio::join!(consume, check);
        consumed?;
        checked?;
        publisher.close().await?;
        // Subscription cancellation leaves the authenticated peer usable.
        assert!(!pair.b.peer_id().to_string().is_empty());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_idle_refresh_rediscovers_restarted_home_product() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path().join("home.json"))?;
        let home = HomeConnection::direct(&pair.a);
        for _ in 0..2 {
            let mut publisher = ScenePublisher::mount(&pair.a)?;
            publisher.publish(&store.built)?;
            let mut count = 0;
            tokio::time::timeout(
                Duration::from_secs(10),
                receive_portals(
                    &pair.b,
                    &home,
                    &CancellationToken::new(),
                    Duration::from_millis(100),
                    |_| count += 1,
                ),
            )
            .await??;
            assert_eq!(count, 1);
            publisher.close().await?;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_rejects_wrong_transport_domain_without_delivering_a_scene() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let mut home = HomeConnection::direct(&pair.a);
    home.domain_id = uuid::Uuid::new_v4().to_string();
    let result = receive_portals(
        &pair.b,
        &home,
        &CancellationToken::new(),
        Duration::from_millis(100),
        |_| panic!("wrong-Domain data reached Swift"),
    )
    .await;
    assert!(result.is_err());
    pair.shutdown().await
}
