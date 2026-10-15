#[allow(dead_code)]
mod support;
use anyhow::Result;
use auki_component_protocol::{CatalogResponse, ComponentProtocolClient, ObservationRequest, ObservationSelection};
use spatial_home::{capture_time, network::ScenePublisher};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_clock_is_discoverable_before_a_scene_and_matches_scene_timestamps() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let mut publisher = ScenePublisher::mount(&pair.a)?;
        let definitions = spatial_home::product_registry::Definitions::default();
        publisher.register_clocks(&definitions)?;
        let registry = spatial_home::product_registry::mount(&pair.a, publisher.runtime.clone(), definitions)?;
        let client = ComponentProtocolClient::new(pair.b.protocols());
        let route = pair.a.listen_addresses()[0].clone();
        let CatalogResponse::Snapshot { snapshot } = client.catalog_exact(pair.a.peer_id(), route.clone(), None).await? else {
            panic!("Missing Catalog");
        };
        let time = snapshot.products.iter().find(|p| p.metadata.as_ref().is_some_and(|m| m.schema == capture_time::METADATA)).unwrap();
        let contract: capture_time::Contract = serde_json::from_value(time.metadata.as_ref().unwrap().value.clone())?;
        let output = snapshot.components.iter().flat_map(|c| c.current_outputs.values())
            .find(|o| o.manifest.reference() == time.manifest.producer).unwrap();
        contract.validate(&output.manifest)?;
        assert_eq!(contract.from_clock, publisher.session_clock.registry_entry());
        assert!(matches!(contract.from_clock.body, auki_registry::ClockBody::MonotonicClock(_)));
        assert!(contract.utc_accuracy_ns.is_none());
        let clock_client = auki_protocols::registry::RegistryClient::new(pair.b.protocols());
        let source = clock_client.fetch_clock_exact(pair.a.peer_id(), route.clone(), contract.from_clock.clock_id.clone(), contract.from_clock.hash()).await?;
        assert_eq!(source, contract.from_clock);
        let destination = clock_client.fetch_clock_exact(pair.a.peer_id(), route.clone(), contract.to_clock.clock_id.clone(), contract.to_clock.hash()).await?;
        assert_eq!(destination, contract.to_clock);
        let initial = client.observations_exact::<capture_time::Sample>(pair.a.peer_id(), route.clone(), ObservationRequest {
            product: time.manifest.reference(), selection: ObservationSelection::LatestExisting,
        }).await?;
        assert_eq!(initial.observations.len(), 1);
        assert!(initial.observations[0].timestamp_ns <= publisher.session_clock.now_ns());
        let dir = tempfile::tempdir()?;
        let store = spatial_home::model::Store::open(dir.path().join("scene.json"))?;
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let before = publisher.session_clock.now_ns();
        publisher.publish(&store.built)?;
        let after = publisher.session_clock.now_ns();
        let catalog = publisher.runtime.catalog().snapshot();
        let scene = catalog.products.iter().find(|p| p.manifest.product_id == spatial_home::network::SCENE_PRODUCT).unwrap();
        let data = client.observations_exact::<spatial_home::network::CanonicalScene>(pair.a.peer_id(), route.clone(), ObservationRequest {
            product: scene.manifest.reference(), selection: ObservationSelection::LatestExisting,
        }).await?;
        assert!((before..=after).contains(&data.observations[0].timestamp_ns));
        assert_eq!(data.producer.clock, spatial_home::model::clock_ref(&contract.from_clock));
        let next = client.observations_exact::<capture_time::Sample>(pair.a.peer_id(), route, ObservationRequest {
            product: time.manifest.reference(), selection: ObservationSelection::LatestExisting,
        }).await?;
        assert_eq!(next.observations[0].sequence, initial.observations[0].sequence,
            "UTC is anchored once at session start, not resampled at publication time");
        registry.close().await?;
        publisher.close().await?;
        let restarted = ScenePublisher::mount(&pair.a)?;
        assert_ne!(restarted.session_clock.registry_entry(), contract.from_clock);
        restarted.close().await?;
        Ok::<_, anyhow::Error>(())
    }.await;
    result.and(pair.shutdown().await.map_err(Into::into))
}
