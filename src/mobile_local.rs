//! Uncalibrated phone-owned geometry. Never feeds the canonical Home mapper.
use anyhow::{Result, anyhow, ensure};
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ComponentProtocolEndpoint,
};
use auki_components::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
pub const SCHEMA: &str = "spatial.phone-local-voxels/v1";
pub const RECEIVER: &str = "home-phone-local-scenes";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub session: String,
    pub timestamp_ns: u64,
    pub cells: Vec<[i32; 3]>,
}
impl ContractType for Snapshot {
    const DATATYPE: &'static str = SCHEMA;
}
impl Snapshot {
    pub fn validate(&self) -> Result<()> {
        uuid::Uuid::parse_str(&self.session)?;
        ensure!(
            self.timestamp_ns > 0 && self.cells.len() <= 10000,
            "Invalid local map size/time"
        );
        ensure!(
            self.cells
                .iter()
                .flatten()
                .all(|x| x.unsigned_abs() <= 10000),
            "Local map out of bounds"
        );
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub source: ProductReference,
    pub snapshot: Snapshot,
}
impl ContractType for Envelope {
    const DATATYPE: &'static str = "spatial.phone-local-envelope/v1";
}
type Cache = BTreeMap<String, (Envelope, Instant)>;
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn prune(c: &mut Cache) {
    c.retain(|_, (_, t)| t.elapsed() < Duration::from_secs(10));
}
pub fn sources() -> serde_json::Value {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    prune(&mut c);
    serde_json::json!(c.keys().map(|id| serde_json::json!({"id":format!("phone:{id}"),"name":"Phone · local voxels (unaligned)","peer_id":id})).collect::<Vec<_>>())
}
pub fn render(peer: &str) -> Result<serde_json::Value> {
    let mut c = cache()
        .lock()
        .map_err(|_| anyhow!("Phone map cache unavailable"))?;
    prune(&mut c);
    let (e, _) = c
        .get(peer)
        .ok_or_else(|| anyhow!("Phone map publisher unavailable"))?;
    render_snapshot(&e.snapshot, peer)
}

/// Render the phone-owned Product without requiring a copy in Home's push cache.
/// These native-session voxels are never input to Home's collaborative mapper.
pub fn render_snapshot(snapshot: &Snapshot, peer: &str) -> Result<serde_json::Value> {
    snapshot.validate()?;
    Ok(
        serde_json::json!({"frame":format!("phone-local:{peer}:{}",snapshot.session),
        "authority":"Phone local mapper","authority_peer_id":peer,
        "capture_timestamp_ns":snapshot.timestamp_ns,
        "mode":"Phone-owned local map", "convention":"OpenGL · Y up · meters",
        "local_map":true,"voxel_size":0.1,
        "portals":[],"objects":[],"notes":[],"sources":{},"integrated_observations":0,
        "voxels":snapshot.cells.iter().map(|p| serde_json::json!({"center":p.map(|x| (x as f64+0.5)*0.1)})).collect::<Vec<_>>()}),
    )
}
pub fn mount_receiver(
    runtime: &ComponentRuntime,
    endpoint: &ComponentProtocolEndpoint,
) -> Result<()> {
    let component = runtime.component(ComponentSpec::new(RECEIVER).operable(OperableContract {
        name: "ingest".into(),
        instruction: Envelope::DATATYPE.into(),
        result: "bool".into(),
        exposure: Exposure::Cluster,
    }))?;
    let op = component.operable(
        "ingest",
        |ctx| !ctx.caller_peer_id.is_empty(),
        |ctx, e: Envelope| {
            e.snapshot
                .validate()
                .map_err(|e| InvocationError::Rejected(e.to_string()))?;
            if e.source.peer_id != ctx.caller_peer_id
                || e.source.product_id != "phone-local-voxels"
                || e.source.manifest_hash.is_empty()
            {
                return Err(InvocationError::Rejected(
                    "Local map provenance mismatch".into(),
                ));
            }
            let mut c = cache()
                .lock()
                .map_err(|_| InvocationError::TargetUnavailable)?;
            prune(&mut c);
            if !c.contains_key(&ctx.caller_peer_id) && c.len() >= 16 {
                return Err(InvocationError::Rejected(
                    "Phone map capacity reached".into(),
                ));
            }
            if let Some((old, _)) = c.get(&ctx.caller_peer_id) {
                if old.snapshot.session == e.snapshot.session
                    && old.snapshot.timestamp_ns > e.snapshot.timestamp_ns
                {
                    return Ok(false);
                }
            }
            c.insert(ctx.caller_peer_id.clone(), (e, Instant::now()));
            Ok(true)
        },
    )?;
    component.expose()?;
    endpoint.export_operable(&op)?;
    Ok(())
}
pub struct Publisher {
    output: ConfiguredObservable<Snapshot>,
    capture: BufferProductCapture<Snapshot>,
    endpoint: Arc<ComponentProtocolEndpoint>,
    _component: Component,
    _runtime: ComponentRuntime,
}
impl Publisher {
    pub fn new(products: &crate::mobile_products::MobileProducts) -> Result<Self> {
        let runtime = products.runtime.clone();
        let owner = runtime.peer_id().to_string();
        let component = runtime.component(
            ComponentSpec::new("phone-local-mapper").observable(ObservableContract {
                name: "voxels".into(),
                datatype: SCHEMA.into(),
                schema: SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }),
        )?;
        let clock = crate::model::clock_entry(&owner, &uuid::Uuid::new_v4().to_string());
        let output = component.configured_observable(ConfiguredObservableSpec::new(
            "voxels",
            uuid::Uuid::new_v4().to_string(),
            crate::model::clock_ref(&clock),
            PayloadContract::Structured(StructuredPayloadContract {
                modality: "scenegraph".into(),
                datatype: SCHEMA.into(),
                schema: SCHEMA.into(),
                observes: "uncalibrated_phone_local_voxels".into(),
                unit: Some("meters".into()),
            }),
        ))?;
        component.expose()?;
        let capture = runtime.capture_buffer(
            "phone-local-voxels",
            &output,
            BufferLimits::entries(1),
            |s: &Snapshot| serde_json::to_vec(s).map_or(usize::MAX, |v| v.len()),
        )?;
        let endpoint = products.endpoint.clone();
        endpoint.export_product(&capture.product())?;
        Ok(Self {
            output,
            capture,
            endpoint,
            _component: component,
            _runtime: runtime,
        })
    }
    pub fn publish(&self, snapshot: Snapshot) -> Result<Envelope> {
        snapshot.validate()?;
        self.output
            .publish(snapshot.timestamp_ns, Arc::new(snapshot.clone()))?;
        ensure!(self.capture.errors().is_empty(), "Local map capture failed");
        Ok(Envelope {
            source: self.capture.product().reference(),
            snapshot,
        })
    }
    pub async fn close(self) -> Result<()> {
        self.endpoint
            .unexport_product(&self.capture.product().manifest.product_id);
        self.capture.delete()?;
        Ok(())
    }
}
pub async fn send(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
    e: &Envelope,
) -> Result<()> {
    let expected = home.validate(peer)?;
    let client = ComponentProtocolClient::new(peer.protocols());
    // Prefer the relay on isolated Wi-Fi; retain direct routes as fallback.
    let mut routes = home.routes.clone();
    routes.sort_by_key(|r| !r.contains("p2p-circuit"));
    for route in routes {
        let route: auki_sdk::Multiaddr = route.parse()?;
        let Ok(Ok(CatalogResponse::Snapshot { snapshot })) = tokio::time::timeout(
            Duration::from_secs(3),
            client.catalog_exact(expected, route.clone(), None),
        )
        .await
        else {
            continue;
        };
        let component = snapshot
            .components
            .iter()
            .find(|c| c.manifest.component_id == RECEIVER)
            .ok_or_else(|| anyhow!("Home needs local map receiver update"))?;
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            client.invoke_exact::<_, bool>(
                expected,
                route,
                component.manifest.reference(),
                "ingest",
                "phone-local",
                uuid::Uuid::new_v4().to_string(),
                Some(Duration::from_secs(3)),
                e,
            ),
        )
        .await??;
        ensure!(response.result, "Home rejected stale local map");
        return Ok(());
    }
    anyhow::bail!("Local map could not reach Home")
}
