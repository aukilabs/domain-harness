//! One SDK service owner per phone connection; publishers only export Products.
use anyhow::{Result, anyhow};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::ComponentRuntime;
use std::sync::Arc;
use std::sync::Mutex;
use auki_protocols::registry::{RegistryEndpoint,v3::*};

pub struct MobileProducts {
    pub runtime: ComponentRuntime,
    pub endpoint: Arc<ComponentProtocolEndpoint>,
    registry: RegistryEndpoint,
    pub definitions: Arc<Mutex<Vec<RegistryEntryEnvelope>>>,
}
impl MobileProducts {
    pub fn mount(peer: &auki_sdk::AukiPeer) -> Result<Self> {
        let runtime = ComponentRuntime::new(peer.peer_id().to_string());
        let endpoint = Arc::new(ComponentProtocolEndpoint::mount(
            peer.protocols(),
            runtime.clone(),
        )?);
        let definitions=Arc::new(Mutex::new(Vec::<RegistryEntryEnvelope>::new()));
        let entries=definitions.clone();
        let registry=RegistryEndpoint::mount(peer.protocols(),move |_:&auki_sdk::AuthenticatedPeer,request:&RegistryRequest|{
            let Ok(entries)=entries.lock() else {return RegistryResponse::Error{reason:"Registry unavailable".into()}};
            match request {
                RegistryRequest::List{kind}=>RegistryResponse::List{entries:entries.iter().filter(|e|e.kind==*kind).map(|e|RegistryListEntry{id:e.id.clone(),hash:e.hash.clone()}).collect()},
                RegistryRequest::Get{kind,id,hash}=>RegistryResponse::Get{entry:entries.iter().find(|e|e.kind==*kind&&e.id==*id&&e.hash==*hash).cloned()},
            }
        })?;
        Ok(Self { runtime, endpoint, registry, definitions })
    }
    /// Publishers must be closed/dropped before the connection's service owner.
    pub async fn close(self) -> Result<()> {
        self.registry.close().await?;
        Arc::try_unwrap(self.endpoint)
            .map_err(|_| anyhow!("Phone publishers still hold the SDK endpoint"))?
            .close()
            .await?;
        Ok(())
    }
}
