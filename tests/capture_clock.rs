#[allow(dead_code)]
mod support;
use anyhow::{Result, ensure};
use auki_components::*;
use spatial_home::{capture_time, product_registry as registry};
use std::time::Duration;

/// The contract is discoverable and consumed over the actual authenticated SDK
/// path, independent of peer names, camera names or robot configuration.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sdk_clock_catalog_and_shared_utc_history_round_trip() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let owner = pair.b.peer_id().to_string();
        let runtime = ComponentRuntime::new(owner.clone());
        let endpoint = auki_component_protocol::ComponentProtocolEndpoint::mount(
            pair.b.protocols(),
            runtime.clone(),
        )?;
        let clock = auki_time::SessionClock::new(&owner, "test-session", "capture");
        let mut publisher =
            capture_time::Publisher::mount(&runtime, &endpoint, clock.registry_entry())?;
        let definitions = registry::Definitions::default();
        registry::clock(&definitions, &publisher.contract.from_clock)?;
        registry::clock(&definitions, &publisher.contract.to_clock)?;
        let registry_endpoint = registry::mount(&pair.b, runtime.clone(), definitions)?;
        publisher.sample(&capture_time::SessionUtcClock(clock.clone()))?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let capture = clock.now_ns();
        assert!(!publisher.sample(&capture_time::SessionUtcClock(clock.clone()))?);
        let routes = pair
            .b
            .listen_addresses()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let mut row =
            spatial_home::device_catalogs::fetch(&pair.a, pair.b.peer_id(), &routes).await;
        row["registries"] =
            registry::resolve(&pair.a, pair.b.peer_id(), &row, None, &Default::default()).await;
        ensure!(row["registries"]["status"] == "ready", "{row}");
        let catalog: CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
        assert_eq!(catalog.products.len(), 1);
        assert_eq!(
            catalog.products[0].metadata.as_ref().unwrap().schema,
            capture_time::METADATA
        );
        let (send, receive) =
            tokio::sync::watch::channel(std::collections::BTreeMap::from([(owner.clone(), row.clone())]));
        let history = spatial_home::time_history::History::default();
        let stop = tokio_util::sync::CancellationToken::new();
        let reader = history.serve(&pair.a, receive, &stop);
        let clock_ref = spatial_home::model::clock_ref(&clock.registry_entry());
        let work = async {
            let value = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let value = history.describe(&clock_ref, capture);
                    if value["status"] == "resolved" {
                        break value;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await?;
            assert!(
                value["timestamp_ns"].as_str().unwrap().parse::<u64>()? > 1_000_000_000_000_000_000
            );
            assert!(value["utc_accuracy_ns"].is_null());
            assert_eq!(
                value["clock"],
                serde_json::to_value(spatial_home::model::clock_ref(&publisher.contract.to_clock))?
            );
            // The session-start relationship remains true for already retained
            // captures while discovery/subscriptions reconnect. It says nothing
            // about whether the peer, camera or pose is currently live.
            let mut unavailable = row.clone();
            unavailable["catalog_status"] = serde_json::json!("unavailable");
            send.send(std::collections::BTreeMap::from([(owner.clone(), unavailable)]))?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(history.describe(&clock_ref, capture)["timestamp_ns"], value["timestamp_ns"]);
            send.send(Default::default())?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(history.describe(&clock_ref, capture)["timestamp_ns"], value["timestamp_ns"]);
            send.send(std::collections::BTreeMap::from([(owner.clone(), row.clone())]))?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(history.describe(&clock_ref, capture)["timestamp_ns"], value["timestamp_ns"]);
            let next = spatial_home::model::clock_ref(&auki_time::SessionClock::new(&owner, "new-session", "capture").registry_entry());
            assert_eq!(history.describe(&next, capture)["status"], "unavailable");
            Ok::<_, anyhow::Error>(())
        };
        let (_, result) = tokio::join!(reader, async {
            let result = work.await;
            stop.cancel();
            result
        });
        registry_endpoint.close().await?;
        endpoint.close().await?;
        result
    }
    .await;
    result.and(pair.shutdown().await)
}
