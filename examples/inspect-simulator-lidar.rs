//! Read-only live smoke test using an ephemeral authenticated consumer identity.
use anyhow::{Result, anyhow};
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ObservationStart, RemoteObservationEvent,
};
use auki_components::{BufferLimits, ProductReference};
use spatial_home::{
    peer::PeerSettings,
    simulated_lidar::{PRODUCT, Scan},
};
use std::{path::Path, time::Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "data/simulator-peer.json".into());
    let identity = tempfile::tempdir()?;
    let mut settings = PeerSettings::load(Path::new(&path))?;
    settings.identity_file = identity.path().join("inspector.identity");
    settings.listen = vec!["/ip4/127.0.0.1/tcp/0".into()];
    settings.advertise.clear();
    settings.advertise_discovery = false;
    settings.relay = false;
    let running = settings
        .start_with_discovery(Some(auki_sdk::DdsTrackerMode::DiscoverOnly))
        .await?;
    let result = async {
        let client = ComponentProtocolClient::new(running.peer.protocols());
        for candidate in running.peer.discover().await? {
            for route in candidate.routes() {
                let Ok(Ok(CatalogResponse::Snapshot { snapshot })) = tokio::time::timeout(
                    Duration::from_secs(2),
                    client.catalog_exact(candidate.peer_id(), route.clone(), None),
                )
                .await
                else {
                    continue;
                };
                let Some(p) = snapshot
                    .products
                    .iter()
                    .find(|p| p.manifest.product_id == PRODUCT)
                else {
                    continue;
                };
                let mut stream = client
                    .subscribe_product_exact(
                        candidate.peer_id(),
                        route.clone(),
                        ProductReference {
                            peer_id: candidate.peer_id().to_string(),
                            product_id: PRODUCT.into(),
                            manifest_hash: p.manifest_hash.clone(),
                        },
                        ObservationStart::NewOnly,
                        BufferLimits::entries(1),
                        |s: &Scan| s.ranges_m.len() * 8,
                    )
                    .await?;
                let read = async {
                    for _ in 0..3 {
                        let Some(RemoteObservationEvent::Observation(o)) = stream.next().await?
                        else {
                            return Err(anyhow!("Expected LiDAR observation"));
                        };
                        o.payload.validate()?;
                        println!(
                            "LiDAR sequence={} timestamp={} beams={} hits={} world_origin={:?}",
                            o.sequence,
                            o.timestamp_ns,
                            o.payload.ranges_m.len(),
                            o.payload.local_hits().len(),
                            o.payload.world_position
                        );
                    }
                    Ok(())
                };
                let result = tokio::time::timeout(Duration::from_secs(5), read).await?;
                stream.close().await?;
                return result;
            }
        }
        Err(anyhow!("No discovered simulator LiDAR Product"))
    }
    .await;
    let close = running.shutdown().await;
    result.and(close)
}
