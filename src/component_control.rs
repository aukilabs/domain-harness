//! Catalog-selected controls, carried only by authenticated SDK operations.
use anyhow::{Result, anyhow, ensure};
use auki_components::{CatalogSnapshot, ContractType};
use crate::component_enabled::{SetEnabled, State, OPERATION};
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub peer_id: String,
    pub component_id: String,
    pub manifest_hash: String,
    pub enabled: bool,
}
pub struct Request {pub command:Command,pub reply:oneshot::Sender<Result<State>>}
pub async fn invoke(client:&auki_component_protocol::ComponentProtocolClient,
    catalogs:&crate::device_catalogs::Catalogs,command:Command)->Result<State> {
    let row=catalogs.get(&command.peer_id).ok_or_else(||anyhow!("Peer unavailable"))?;
    ensure!(row["catalog_status"]=="ready","Peer Catalog unavailable");
    let catalog:CatalogSnapshot=serde_json::from_value(row["catalog"].clone())?;
    let component=catalog.components.iter().find(|c|c.manifest.component_id==command.component_id
        && c.manifest_hash==command.manifest_hash).ok_or_else(||anyhow!("Component changed; refresh Devices"))?;
    ensure!(component.manifest.operables.iter().any(|o|o.name==OPERATION
        && o.instruction==SetEnabled::DATATYPE && o.result==State::DATATYPE),"Component does not support set_enabled");
    let reference=component.manifest.reference();
    ensure!(reference.peer_id==command.peer_id,"Catalog owner mismatch");
    let result=client.invoke_exact::<_,State>(command.peer_id.parse()?,
        row["catalog_route"].as_str().ok_or_else(||anyhow!("Peer route unavailable"))?.parse()?,
        reference,OPERATION,"home-device-controls",&uuid::Uuid::new_v4().to_string(),
        Some(Duration::from_secs(5)),&SetEnabled {enabled:command.enabled}).await?;
    Ok(result.result)
}
pub async fn serve(peer:&auki_sdk::AukiPeer,catalogs:watch::Receiver<crate::device_catalogs::Catalogs>,
    mut requests:mpsc::Receiver<Request>,stop:&tokio_util::sync::CancellationToken) {
    let client=auki_component_protocol::ComponentProtocolClient::new(peer.protocols());
    loop {
        let request=tokio::select!{_ = stop.cancelled()=>break,r=requests.recv()=>match r {Some(r)=>r,None=>break}};
        let snapshot=catalogs.borrow().clone();
        let result=tokio::select!{_ = stop.cancelled()=>break,r=tokio::time::timeout(Duration::from_secs(6),invoke(&client,&snapshot,request.command))=>r.unwrap_or_else(|_|Err(anyhow!("Control timed out; state is unconfirmed. Refresh before retrying.")))};
        let _=request.reply.send(result);
    }
}
