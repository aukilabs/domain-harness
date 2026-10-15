//! Server-owned traffic sampling. Browser visibility never affects collection.
use serde::{Deserialize, Serialize};
use std::{path::{Path, PathBuf}, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Sample {
    pub elapsed_ms: f64,
    pub interval_ms: f64,
    pub received: f64,
    pub sent: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Launch {
    #[serde(default)]
    pub observer_peer_id:Option<String>,
    #[serde(default)]
    pub remote_peer_id:Option<String>,
    #[serde(default)]
    pub read_errors:u64,
    #[serde(default)]
    pub write_errors:u64,
    #[serde(default)]
    pub opened_streams:u64,
    #[serde(default)]
    pub active_streams:u64,
    #[serde(default)]
    pub links:std::collections::BTreeMap<String,Box<Launch>>,
    pub session: String,
    pub started_at: String,
    pub last_sample_at: String,
    pub ended_cleanly: bool,
    pub elapsed_ms: f64,
    pub received_bytes: u64,
    pub sent_bytes: u64,
    pub peak_received: Sample,
    pub peak_sent: Sample,
    pub samples: Vec<Sample>,
    pub top_intervals: Vec<Sample>,
}
impl Launch {
    fn new() -> Self {
        let now=chrono::Utc::now().to_rfc3339();
        Self {observer_peer_id:None,remote_peer_id:None,read_errors:0,write_errors:0,opened_streams:0,active_streams:0,links:Default::default(),session:uuid::Uuid::new_v4().to_string(),started_at:now.clone(),last_sample_at:now,
            ended_cleanly:false,elapsed_ms:0.,received_bytes:0,sent_bytes:0,
            peak_received:Sample::default(),peak_sent:Sample::default(),samples:vec![],top_intervals:vec![]}
    }
    fn record(&mut self, elapsed_ms:f64, received:u64, sent:u64) {
        let interval_ms=elapsed_ms-self.elapsed_ms;
        if interval_ms<=0. {return;}
        let sample=Sample {elapsed_ms,interval_ms,
            received:received.saturating_sub(self.received_bytes) as f64*1000./interval_ms,
            sent:sent.saturating_sub(self.sent_bytes) as f64*1000./interval_ms};
        self.elapsed_ms=elapsed_ms;self.received_bytes=received;self.sent_bytes=sent;
        self.last_sample_at=chrono::Utc::now().to_rfc3339();
        if sample.received>self.peak_received.received {self.peak_received=sample.clone();}
        if sample.sent>self.peak_sent.sent {self.peak_sent=sample.clone();}
        self.samples.push(sample.clone());
        if self.samples.len()>300 {self.samples.remove(0);}
        if sample.received+sample.sent>0. {
            self.top_intervals.push(sample);
            self.top_intervals.sort_by(|a,b|(b.received+b.sent).total_cmp(&(a.received+a.sent)));
            self.top_intervals.truncate(10);
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Archive {pub version:u32,pub launches:Vec<Launch>}
#[derive(Clone, Serialize)]
pub struct Diagnostics {
    pub current:Launch,
    pub prior:Vec<Launch>,
    pub persistence_error:Option<String>,
    pub saved_at:Option<String>,
}
pub struct Monitor {state:Mutex<Diagnostics>,path:PathBuf,start:Instant}
impl Monitor {
    pub fn open(path:PathBuf)->Self {
        let (prior,error)=match std::fs::read(&path) {
            Ok(bytes)=>match serde_json::from_slice::<Archive>(&bytes) {
                Ok(mut archive) if archive.version==1=>{archive.launches.truncate(19);(archive.launches,None)},
                _=>(vec![],Some("Existing traffic history is unreadable; it has been preserved. Export this session before restarting.".into())),
            },
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>(vec![],None),
            Err(_)=>(vec![],Some("Traffic history could not be read; recording in memory only.".into())),
        };
        Self {state:Mutex::new(Diagnostics {current:Launch::new(),prior,persistence_error:error,saved_at:None}),path,start:Instant::now()}
    }
    pub fn snapshot(&self)->Diagnostics {self.state.lock().unwrap_or_else(|e|e.into_inner()).clone()}
    pub fn peer_snapshot(&self,owner:&str,remote:&str)->Option<Diagnostics> {
        let all=self.snapshot();let key=format!("{owner}/{remote}");
        let current=(**all.current.links.get(&key)?).clone();
        let prior=all.prior.into_iter().filter_map(|run|run.links.get(&key).map(|value|(**value).clone())).collect();
        Some(Diagnostics{current,prior,persistence_error:all.persistence_error,saved_at:all.saved_at})
    }
    fn sample(&self) {
        let (rx,tx)=auki_p2p::application_traffic_totals();
        let elapsed=self.start.elapsed().as_secs_f64()*1000.;
        let (links,_)=auki_p2p::peer_traffic_snapshot();
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        let previous_elapsed=state.current.elapsed_ms;
        state.current.record(elapsed,rx,tx);
        for link in links {
            let key=format!("{}/{}",link.local_peer_id,link.remote_peer_id);
            if !state.current.links.contains_key(&key){
                let mut run=Launch::new();run.session=state.current.session.clone();run.started_at=state.current.started_at.clone();run.elapsed_ms=previous_elapsed;
                run.observer_peer_id=Some(link.local_peer_id.clone());run.remote_peer_id=Some(link.remote_peer_id.clone());
                state.current.links.insert(key.clone(),Box::new(run));
            }
            let run=state.current.links.get_mut(&key).unwrap();run.record(elapsed,link.received_bytes,link.sent_bytes);
            // Keep a bounded recent minute per link; launch peaks survive eviction.
            if run.samples.len()>60 {run.samples.remove(0);}
            run.read_errors=link.read_errors;run.write_errors=link.write_errors;run.opened_streams=link.opened_streams;run.active_streams=link.active_streams;
        }
    }
    fn save(&self, preserve_existing:bool) {
        if preserve_existing {return;}
        let snapshot=self.snapshot();
        let mut launches=vec![snapshot.current];launches.extend(snapshot.prior);launches.truncate(20);
        let result=save_archive(&self.path,&Archive {version:1,launches});
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        match result {
            Ok(())=>{state.saved_at=Some(chrono::Utc::now().to_rfc3339());state.persistence_error=None;},
            Err(_)=>state.persistence_error=Some("History save failed; recording in memory. Export before restarting.".into()),
        }
    }
    pub async fn run(self:Arc<Self>, stop:CancellationToken) {
        let preserve_existing=self.snapshot().persistence_error.is_some();
        self.save(preserve_existing);
        let mut ticker=tokio::time::interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        let mut ticks=0;
        loop {
            tokio::select! {
                _=stop.cancelled()=>break,
                _=ticker.tick()=>{self.sample();ticks+=1;if ticks%5==0 {self.save(preserve_existing);}}
            }
        }
        self.sample();{
            let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());state.current.ended_cleanly=true;
            for link in state.current.links.values_mut(){link.ended_cleanly=true;}
        }
        self.save(preserve_existing);
    }
}
fn save_archive(path:&Path,archive:&Archive)->anyhow::Result<()> {
    let temporary=path.with_extension("traffic.tmp");
    let bytes=serde_json::to_vec(archive)?;
    use std::io::Write;
    let mut file=std::fs::File::create(&temporary)?;file.write_all(&bytes)?;file.sync_all()?;
    std::fs::rename(temporary,path)?;Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peaks_use_elapsed_time_and_keep_bounded_ranked_history() {
        let mut launch=Launch::new();launch.record(2000.,4000,2000);
        assert_eq!(launch.peak_received.received,2000.);
        launch.record(3000.,9000,3000);assert_eq!(launch.peak_received.elapsed_ms,3000.);
        for i in 4..=400 {launch.record(i as f64*1000.,9000,3000);}
        assert_eq!(launch.samples.len(),300);assert_eq!(launch.peak_received.received,5000.);
        assert_eq!(launch.top_intervals[0].received,5000.);
        assert_eq!(launch.sent_bytes,3000);
    }
    #[test]
    fn restart_retains_prior_launch_without_reusing_its_counters() {
        let dir=std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());std::fs::create_dir(&dir).unwrap();
        let path=dir.join("traffic.json");let monitor=Monitor::open(path.clone());
        monitor.state.lock().unwrap().current.record(1000.,9000,4000);monitor.save(false);
        let restarted=Monitor::open(path);let snapshot=restarted.snapshot();
        assert_eq!(snapshot.prior[0].received_bytes,9000);assert_eq!(snapshot.current.received_bytes,0);
        assert_ne!(snapshot.current.session,snapshot.prior[0].session);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn peer_history_never_falls_back_to_aggregate_or_another_peer() {
        let monitor=Monitor::open(std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()));
        {let mut state=monitor.state.lock().unwrap();
         state.current.record(1000.,99999,99999);
         let mut phone=Launch::new();phone.observer_peer_id=Some("home".into());phone.remote_peer_id=Some("phone".into());phone.record(1000.,50,100);
         state.current.links.insert("home/phone".into(),Box::new(phone));
         state.prior.push(Launch::new());}
        let phone=monitor.peer_snapshot("home","phone").unwrap();assert_eq!(phone.current.sent_bytes,100);assert!(phone.prior.is_empty());
        assert!(monitor.peer_snapshot("other-home","phone").is_none());assert!(monitor.peer_snapshot("home","robot").is_none());
    }
    #[test]
    fn corrupt_history_is_reported_and_preserved() {
        let path=std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());std::fs::write(&path,b"broken").unwrap();
        let monitor=Monitor::open(path.clone());assert!(monitor.snapshot().persistence_error.is_some());
        monitor.save(true);assert_eq!(std::fs::read(&path).unwrap(),b"broken");std::fs::remove_file(path).unwrap();
    }
}
