//! Network diagnostics are an SDK Observable, scoped to the observing peer.
use anyhow::{Result,ensure};
use auki_components::*;
use auki_components::clock::ClockRegistryEntry;
use auki_component_protocol::ComponentProtocolEndpoint;
use serde::{Serialize,Deserialize};
use std::{sync::{Arc,OnceLock},time::{Instant,Duration}};
pub const SCHEMA:&str="spatial-harness.peer-traffic/v1";
pub const PRODUCT:&str="peer-network-traffic";
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Link {
    pub observer_peer_id:String,
    pub remote_peer_id:String,
    pub received_bytes:u64,
    pub sent_bytes:u64,
    pub read_errors:u64,
    pub write_errors:u64,
    pub opened_streams:u64,
    pub active_streams:u64,
}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Snapshot {
    pub observer_peer_id:String,
    pub counter_session:String,
    pub scope:String,
    pub links:Vec<Link>,
    pub untracked_streams:u64,
}
impl ContractType for Snapshot {const DATATYPE:&'static str=SCHEMA;}
pub fn snapshot(owner:&str)->Snapshot {
    static SESSION:OnceLock<String>=OnceLock::new();
    let (links,untracked_streams)=auki_p2p::peer_traffic_snapshot();
    Snapshot {observer_peer_id:owner.into(),counter_session:SESSION.get_or_init(||uuid::Uuid::new_v4().to_string()).clone(),
        scope:"authenticated_application_stream_bytes; directions relative to observer; successful writes are not delivery acknowledgements; errors are I/O operations, not packet loss".into(),
        links:links.into_iter().filter(|l|l.local_peer_id==owner).map(|l|Link {observer_peer_id:l.local_peer_id,remote_peer_id:l.remote_peer_id,received_bytes:l.received_bytes,sent_bytes:l.sent_bytes,read_errors:l.read_errors,write_errors:l.write_errors,opened_streams:l.opened_streams,active_streams:l.active_streams}).collect(),untracked_streams}
}
pub struct Publisher {
    _component:Component,
    capture:BufferProductCapture<Snapshot>,
    output:ConfiguredObservable<Snapshot>,
    owner:String,
    started:Instant,
    last:Option<Instant>,
}
impl Publisher {
    pub fn mount(runtime:&ComponentRuntime,endpoint:&ComponentProtocolEndpoint,clock:&ClockRegistryEntry,started:Instant)->Result<Self>{
        let component=runtime.component(ComponentSpec::new("peer-network-diagnostics").observable(ObservableContract {
            name:"traffic".into(),datatype:SCHEMA.into(),schema:SCHEMA.into(),access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster,
        }))?;
        let output=component.configured_observable(ConfiguredObservableSpec::new("traffic",format!("traffic-{}",clock.session_id),crate::model::clock_ref(clock),
            PayloadContract::Structured(StructuredPayloadContract{modality:"network_traffic".into(),datatype:SCHEMA.into(),schema:SCHEMA.into(),observes:"authenticated_peer_link_traffic".into(),unit:Some("bytes".into())})))?;
        component.expose()?;
        let capture=runtime.capture_buffer(PRODUCT,&output,BufferLimits::entries(2),|s:&Snapshot|512+s.links.len()*512)?;
        endpoint.export_product(&capture.product())?;
        let mut result=Self{_component:component,capture,output,owner:clock.peer_id.clone(),started,last:None};result.tick()?;Ok(result)
    }
    pub fn tick(&mut self)->Result<()> {
        if self.last.is_some_and(|last|last.elapsed()<Duration::from_secs(1)){return Ok(());}
        self.last=Some(Instant::now());
        self.output.publish(self.started.elapsed().as_nanos() as u64,Arc::new(snapshot(&self.owner)))?;
        ensure!(self.capture.errors().is_empty(),"Traffic retention failed");Ok(())
    }
}
