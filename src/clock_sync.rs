//! Peer-independent clock bindings for bounded capture-time exchanges.
//! UTC anchors stay immutable. A measured inter-peer offset does not claim
//! either machine's wall clock is globally synchronized.
use crate::capture_time::{Contract, Sample};
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{CatalogResponse, ComponentProtocolClient, ObservationRequest, ObservationSelection};
use auki_components::ClockReference;
use auki_protocols::registry::RegistryClient;
use auki_registry::ClockRegistryEntry;
use auki_sdk::{Multiaddr, PeerId};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub fn reference(clock: &ClockRegistryEntry) -> ClockReference {
    ClockReference { peer_id: clock.peer_id.clone(), id: clock.clock_id.clone(), hash: clock.hash() }
}

#[derive(Clone, Debug)]
pub struct RegisteredClock {
    pub contract: Contract,
    pub anchor: (u64, Sample),
}
impl RegisteredClock {
    pub fn local(publisher: &crate::capture_time::Publisher) -> Result<Self> {
        Ok(Self { contract: publisher.contract.clone(), anchor: publisher.anchor()? })
    }
    pub fn reference(&self) -> ClockReference { reference(&self.contract.from_clock) }
    fn utc(&self, at: u64) -> Result<(u64, u64)> {
        crate::capture_time::to_utc(&self.contract, std::slice::from_ref(&self.anchor), at)
    }
}

/// Select by exact stream clock, never by peer type or the first clock in a
/// Catalog. Resolve both definitions through Registry and read sequence zero
/// from the actual SDK Product; metadata alone is not an observation.
pub async fn resolve(client: &ComponentProtocolClient, registry: &RegistryClient,
    peer: PeerId, route: Multiaddr, clock: &ClockReference) -> Result<RegisteredClock> {
    tokio::time::timeout(Duration::from_secs(8), async {
        ensure!(clock.peer_id == peer.to_string(), "Clock belongs to another peer");
        let CatalogResponse::Snapshot { snapshot } = client.catalog_exact(peer, route.clone(), None).await?
            else { anyhow::bail!("Clock Catalog unavailable") };
        let mut matches = Vec::new();
        for product in &snapshot.products {
            let Some(metadata) = &product.metadata else { continue };
            if metadata.schema != crate::capture_time::METADATA { continue; }
            let contract: Contract = serde_json::from_value(metadata.value.clone())?;
            if reference(&contract.from_clock) != *clock { continue; }
            let output = snapshot.components.iter().flat_map(|c| c.current_outputs.values())
                .find(|o| o.manifest.reference() == product.manifest.producer)
                .ok_or_else(|| anyhow!("Clock Product Output missing"))?;
            contract.validate(&output.manifest)?;
            ensure!(product.manifest.peer_id == peer.to_string() && product.manifest.hash() == product.manifest_hash,
                "Clock Product identity mismatch");
            matches.push((product, contract, &output.manifest, metadata.source_sequence));
        }
        ensure!(matches.len() == 1, "Missing or ambiguous UTC-transform Product for stream clock");
        let (product, contract, output, sequence) = matches.remove(0);
        for definition in [&contract.from_clock, &contract.to_clock] {
            let received = registry.fetch_clock_exact(peer, route.clone(), &definition.clock_id, &definition.hash()).await?;
            ensure!(received == *definition, "Clock Registry definition mismatch");
        }
        let received = client.observations_exact::<Sample>(peer, route, ObservationRequest {
            product: product.manifest.reference(), selection: ObservationSelection::LatestExisting,
        }).await?;
        ensure!(received.product == product.manifest && received.product_manifest_hash == product.manifest_hash
            && received.producer == *output, "Clock Product changed during discovery");
        ensure!(received.observations.len() == 1, "Session UTC anchor unavailable");
        let observation = &received.observations[0];
        ensure!(sequence == 0 && observation.sequence == 0 && observation.output == output.reference(),
            "Session UTC anchor was replaced");
        Ok(RegisteredClock { contract, anchor: (observation.timestamp_ns, (*observation.payload).clone()) })
    }).await.map_err(|_| anyhow!("Clock discovery timed out"))?
}

pub async fn resolve_routes(client: &ComponentProtocolClient, registry: &RegistryClient,
    peer: PeerId, routes: &[String], clock: &ClockReference) -> Result<RegisteredClock> {
    ensure!(!routes.is_empty() && routes.len() <= 16, "Clock discovery needs 1–16 routes");
    let mut last = anyhow!("No clock route available");
    for route in routes {
        match resolve(client, registry, peer, route.parse()?, clock).await {
            Ok(clock) => return Ok(clock),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// All values are under the explicitly named SDK session clocks.
#[derive(Clone, Debug)]
pub struct Probe {
    pub sent_ns: u64,
    pub remote_ns: u64,
    pub received_ns: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub from_clock: ClockReference,
    pub to_clock: ClockReference,
    pub offset_ns: i64,
    pub uncertainty_ns: u64,
    valid_from_ns: u64,
    valid_until_ns: u64,
}
impl Mapping {
    pub fn measure(from: &RegisteredClock, to: &RegisteredClock, probes: &[Probe]) -> Result<Self> {
        ensure!(probes.len() >= 3, "Too few clock exchanges");
        ensure!(probes.iter().all(|p| p.received_ns >= p.sent_ns && p.remote_ns > 0), "Invalid clock exchange");
        ensure!(probes.windows(2).all(|p| p[0].received_ns <= p[1].sent_ns && p[0].remote_ns <= p[1].remote_ns),
            "Clock changed during exchange");
        let best = probes.iter().min_by_key(|p| p.received_ns - p.sent_ns).unwrap();
        let rtt = best.received_ns - best.sent_ns;
        let middle = best.sent_ns + rtt / 2;
        // Account for the observed difference between each host's UTC, without
        // changing either session anchor or pretending the two UTCs agree.
        let (local_utc, a) = from.utc(middle)?;
        let (remote_utc, b) = to.utc(best.remote_ns)?;
        let utc_difference = remote_utc as i128 - local_utc as i128;
        let offset = from.anchor.1.offset_ns as i128 + utc_difference - to.anchor.1.offset_ns as i128;
        let uncertainty = (rtt / 2 + 1).checked_add(a).and_then(|v| v.checked_add(b))
            .and_then(|v| v.checked_add(from.contract.capture_conversion_uncertainty_ns))
            .and_then(|v| v.checked_add(to.contract.capture_conversion_uncertainty_ns))
            .ok_or_else(|| anyhow!("Clock uncertainty overflow"))?;
        Ok(Self { from_clock: from.reference(), to_clock: to.reference(),
            offset_ns: offset.try_into().map_err(|_| anyhow!("Clock offset out of range"))?, uncertainty_ns: uncertainty,
            valid_from_ns: probes[0].sent_ns.saturating_sub(10_000_000_000),
            valid_until_ns: probes.last().unwrap().received_ns.checked_add(120_000_000_000)
                .ok_or_else(|| anyhow!("Clock validity overflow"))? })
    }
    pub fn remote(&self, source: &ClockReference, timestamp: u64) -> Result<u64> {
        ensure!(source == &self.from_clock, "Capture clock changed; restart onboarding");
        ensure!((self.valid_from_ns..=self.valid_until_ns).contains(&timestamp), "Clock measurement expired; restart onboarding");
        let t = timestamp as i128 + self.offset_ns as i128;
        ensure!(t > 0 && t <= u64::MAX as i128, "Clock mapping out of range");
        Ok(t as u64)
    }
}
