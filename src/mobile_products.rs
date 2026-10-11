//! One SDK service owner per phone connection; publishers only export Products.
use anyhow::{Result, anyhow};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::ComponentRuntime;
use std::sync::Arc;

pub struct MobileProducts {
    pub runtime: ComponentRuntime,
    pub endpoint: Arc<ComponentProtocolEndpoint>,
}
impl MobileProducts {
    pub fn mount(peer: &auki_sdk::AukiPeer) -> Result<Self> {
        let runtime = ComponentRuntime::new(peer.peer_id().to_string());
        let endpoint = Arc::new(ComponentProtocolEndpoint::mount(
            peer.protocols(),
            runtime.clone(),
        )?);
        Ok(Self { runtime, endpoint })
    }
    /// Publishers must be closed/dropped before the connection's service owner.
    pub async fn close(self) -> Result<()> {
        Arc::try_unwrap(self.endpoint)
            .map_err(|_| anyhow!("Phone publishers still hold the SDK endpoint"))?
            .close()
            .await?;
        Ok(())
    }
}
