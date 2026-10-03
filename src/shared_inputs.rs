//! One selected observation sequence feeds independently computed maps.
use crate::{network::HomeConnection, runner_voxels::MappingInput};
use anyhow::{Result, anyhow, bail, ensure};
use auki_component_protocol::*;
use auki_components::*;
use auki_sdk::AukiPeer;
use std::sync::Arc;
pub const INPUT_PRODUCT: &str = "accepted-mapping-inputs";
pub const INPUT_SCHEMA: &str = "spatial-harness.mapping-input/v1";
pub struct InputPublisher {
    output: ConfiguredObservable<MappingInput>,
    _component: Component,
    capture: BufferProductCapture<MappingInput>,
    last: Option<usize>,
    timestamp: u64,
}
impl InputPublisher {
    pub fn mount(
        runtime: &ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        clock: ClockReference,
    ) -> Result<Self> {
        let component = runtime.component(
            ComponentSpec::new("accepted-scan-publisher").observable(ObservableContract {
                name: "inputs".into(),
                datatype: INPUT_SCHEMA.into(),
                schema: INPUT_SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }),
        )?;
        let output = component.configured_observable(ConfiguredObservableSpec::new(
            "inputs",
            format!("inputs-{}", uuid::Uuid::new_v4()),
            clock,
            PayloadContract::Structured(StructuredPayloadContract {
                modality: "mapping_observation".into(),
                datatype: INPUT_SCHEMA.into(),
                schema: INPUT_SCHEMA.into(),
                observes: "accepted_lidar_scans".into(),
                unit: Some("meters".into()),
            }),
        ))?;
        component.expose()?;
        let capture = runtime.capture_buffer(
            INPUT_PRODUCT,
            &output,
            BufferLimits {
                max_entries: Some(256),
                max_bytes: Some(16 * 1024 * 1024),
                target_duration: None,
            },
            input_size,
        )?;
        endpoint.export_product(&capture.product())?;
        Ok(Self {
            output,
            _component: component,
            capture,
            last: None,
            timestamp: 0,
        })
    }
    pub fn publish(&mut self, inputs: &[MappingInput], timestamp: u64) -> Result<()> {
        for input in inputs {
            if self.last.is_some_and(|r| input.revision <= r) {
                continue;
            }
            ensure!(
                self.last.is_none_or(|r| input.revision == r + 1),
                "accepted input publication gap"
            );
            ensure!(input_size(input) < 128 * 1024, "mapping input too large");
            self.timestamp = timestamp.max(self.timestamp + 1);
            self.output
                .publish(self.timestamp, Arc::new(input.clone()))?;
            ensure!(
                self.capture.errors().is_empty(),
                "mapping input retention failed"
            );
            self.last = Some(input.revision);
        }
        Ok(())
    }
}
fn input_size(input: &MappingInput) -> usize {
    serde_json::to_vec(input).map_or(usize::MAX, |v| v.len())
}
pub async fn subscribe(
    peer: &AukiPeer,
    home: &HomeConnection,
) -> Result<RemoteProductSubscription<MappingInput>> {
    let expected = home.validate(peer)?;
    let client = ComponentProtocolClient::new(peer.protocols());
    let mut error = anyhow!("No Home route available");
    for route in &home.routes {
        let work = async {
            let CatalogResponse::Snapshot { snapshot } =
                client.catalog_exact(expected, route.parse()?, None).await?
            else {
                bail!("Home catalog unavailable")
            };
            let p = snapshot
                .products
                .iter()
                .find(|p| p.manifest.product_id == INPUT_PRODUCT)
                .ok_or_else(|| anyhow!("Home does not publish accepted mapping inputs"))?;
            let subscription = client
                .subscribe_product_exact(
                    expected,
                    route.parse()?,
                    ProductReference {
                        peer_id: home.peer_id.clone(),
                        product_id: INPUT_PRODUCT.into(),
                        manifest_hash: p.manifest_hash.clone(),
                    },
                    ObservationStart::FromSequence { sequence: 0 },
                    BufferLimits::entries(256),
                    input_size,
                )
                .await?;
            ensure!(
                subscription.product().producer.payload.schema() == INPUT_SCHEMA,
                "unexpected mapping input schema"
            );
            Ok::<_, anyhow::Error>(subscription)
        };
        match tokio::time::timeout(std::time::Duration::from_secs(20), work).await {
            Ok(Ok(s)) => return Ok(s),
            Ok(Err(e)) => error = e,
            Err(e) => error = e.into(),
        }
    }
    Err(error)
}
