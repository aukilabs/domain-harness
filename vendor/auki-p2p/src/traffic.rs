//! Counters keyed by both authenticated transport identities. Application bytes,
//! not wire bytes, delivery acknowledgements, or packet loss.
use std::{collections::BTreeMap, sync::{Arc, Mutex, OnceLock}, task::Poll};

#[derive(Clone, Debug, Default)]
pub struct PeerTraffic {
    pub local_peer_id: String,
    pub remote_peer_id: String,
    pub received_bytes: u64,
    pub sent_bytes: u64,
    pub read_errors: u64,
    pub write_errors: u64,
    pub opened_streams: u64,
    pub active_streams: u64,
}
pub(crate) type Counter = Arc<Mutex<PeerTraffic>>;
#[derive(Default)]
struct Registry { links:BTreeMap<(String,String),Counter>, untracked_streams:u64 }
static REGISTRY:OnceLock<Mutex<Registry>>=OnceLock::new();
fn registry()->&'static Mutex<Registry>{REGISTRY.get_or_init(||Mutex::new(Registry::default()))}
impl Registry {
    fn open(&mut self,local:&str,remote:&str)->Option<Counter>{
        let key=(local.to_owned(),remote.to_owned());
        if !self.links.contains_key(&key)&&self.links.len()>=256 {self.untracked_streams+=1;return None;}
        let counter=self.links.entry(key).or_insert_with(||Arc::new(Mutex::new(PeerTraffic{local_peer_id:local.into(),remote_peer_id:remote.into(),..Default::default()}))).clone();
        {let mut value=counter.lock().unwrap_or_else(|e|e.into_inner());value.opened_streams+=1;value.active_streams+=1;}
        Some(counter)
    }
}
pub(crate) fn open(local:&str,remote:&str)->Option<Counter>{registry().lock().unwrap_or_else(|e|e.into_inner()).open(local,remote)}
pub(crate) fn close(counter:&Option<Counter>){if let Some(counter)=counter {let mut value=counter.lock().unwrap_or_else(|e|e.into_inner());value.active_streams=value.active_streams.saturating_sub(1);}}
pub(crate) fn record(counter:&Option<Counter>,result:&Poll<std::io::Result<usize>>,received:bool){
    if let Some(counter)=counter {let mut v=counter.lock().unwrap_or_else(|e|e.into_inner());match result {
        Poll::Ready(Ok(n))=>{let count=if received {&mut v.received_bytes}else{&mut v.sent_bytes};*count=count.saturating_add(*n as u64);},
        Poll::Ready(Err(_))=>{let count=if received {&mut v.read_errors}else{&mut v.write_errors};*count=count.saturating_add(1);},
        Poll::Pending=>{},
    }}
}
/// Process-lifetime counters per authenticated local/remote identity pair.
/// The second value counts stream opens not attributed after the 256-pair limit.
pub fn peer_traffic_snapshot()->(Vec<PeerTraffic>,u64){
    let r=registry().lock().unwrap_or_else(|e|e.into_inner());
    (r.links.values().map(|v|v.lock().unwrap_or_else(|e|e.into_inner()).clone()).collect(),r.untracked_streams)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separates_both_endpoints_and_directions_and_tracks_stream_lifetime(){
        let mut r=Registry::default();let a=r.open("home","phone");let b=r.open("home","robot");let c=r.open("other-home","phone");
        record(&a,&Poll::Ready(Ok(42)),false);record(&a,&Poll::Ready(Ok(7)),true);
        record(&a,&Poll::Pending,true);record(&a,&Poll::Ready(Err(std::io::Error::other("closed"))),true);
        let value=a.as_ref().unwrap().lock().unwrap().clone();assert_eq!((value.sent_bytes,value.received_bytes,value.read_errors),(42,7,1));
        assert_eq!(b.unwrap().lock().unwrap().sent_bytes,0);assert_eq!(c.unwrap().lock().unwrap().sent_bytes,0);
        close(&a);assert_eq!(a.unwrap().lock().unwrap().active_streams,0);
    }
    #[test]
    fn bounds_identity_retention_without_reassigning_an_existing_counter(){
        let mut r=Registry::default();for n in 0..256 {assert!(r.open("home",&n.to_string()).is_some());}
        assert!(r.open("home","overflow").is_none());assert_eq!(r.untracked_streams,1);
        assert!(r.open("home","0").is_some());assert_eq!(r.links.len(),256);
    }
}
