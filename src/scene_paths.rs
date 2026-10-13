//! Home-authored polylines: metres in an explicit scene frame, never camera pixels.
use anyhow::{Result, ensure, anyhow};
use auki_components::*;
use serde::{Serialize, Deserialize};
use std::{path::PathBuf, sync::{Arc, Mutex}, collections::BTreeSet};
pub const SCHEMA: &str = "spatial-harness.scene-paths/v1";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stroke {
    pub id: String,
    pub title: String,
    pub color: String,
    pub width_m: f64,
    /// Separate segments prevent drawing across a missed raycast.
    pub segments: Vec<Vec<[f64; 3]>>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Paths { pub schema: String, pub frame: String, pub items: Vec<Stroke> }
impl ContractType for Paths { const DATATYPE: &'static str = SCHEMA; }
impl Paths {
    pub fn empty(frame: &str) -> Self { Self { schema: SCHEMA.into(), frame: frame.into(), items: vec![] } }
    pub fn validate(&self, frame: &str) -> Result<()> {
        ensure!(self.schema == SCHEMA && self.frame == frame, "Path frame/schema mismatch");
        ensure!(self.items.len() <= 256, "Too many paths");
        let mut ids = BTreeSet::new();
        let mut total = 0;
        for s in &self.items {
            ensure!(uuid::Uuid::parse_str(&s.id).is_ok() && ids.insert(&s.id), "Invalid/duplicate path ID");
            ensure!(!s.title.trim().is_empty() && s.title.chars().count() <= 120, "Invalid path title");
            ensure!(s.color.len()==7 && s.color.starts_with('#') && s.color[1..].bytes().all(|c|c.is_ascii_hexdigit()), "Invalid path color");
            ensure!(s.width_m.is_finite() && (0.005..=0.25).contains(&s.width_m), "Invalid path width");
            ensure!(!s.segments.is_empty() && s.segments.len()<=128, "Invalid path segments");
            for segment in &s.segments {
                ensure!((2..=4096).contains(&segment.len()), "Path needs 2–4096 points per segment");
                ensure!(segment.iter().flatten().all(|v|v.is_finite() && v.abs()<=100000.), "Invalid path coordinate");
                total += segment.len();
            }
        }
        ensure!(total <= 32768, "Scene path point budget exceeded");
        Ok(())
    }
}
pub struct Storage { directory: PathBuf, writer: Mutex<()> }
impl Storage {
    pub fn new(directory: PathBuf) -> Self { Self { directory, writer:Mutex::new(()) } }
    fn path(&self, scene: &str) -> Result<PathBuf> {
        ensure!(!scene.is_empty() && scene.len()<=64 && scene.bytes().all(|c|c.is_ascii_lowercase()||c.is_ascii_digit()||c==b'-'), "Invalid scene ID");
        Ok(self.directory.join(format!("{scene}.json")))
    }
    pub fn get(&self, scene: &str, frame: &str) -> Result<Paths> {
        let path=self.path(scene)?;
        match std::fs::read(path) {
            Ok(bytes)=>{let value:Paths=serde_json::from_slice(&bytes)?;value.validate(frame)?;Ok(value)},
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(Paths::empty(frame)),
            Err(e)=>Err(e.into()),
        }
    }
    pub fn change(&self, scene: &str, frame: &str, id: &str, stroke: Option<Stroke>) -> Result<Paths> {
        use std::io::Write;
        let _guard=self.writer.lock().map_err(|_|anyhow!("Path storage unavailable"))?;
        let mut paths=self.get(scene,frame)?;
        if let Some(stroke)=stroke {
            ensure!(stroke.id==id, "Path identity mismatch");
            if let Some(existing)=paths.items.iter().find(|s|s.id==id) {
                ensure!(existing==&stroke,"Path already exists with different content");
                return Ok(paths);
            }
            paths.items.push(stroke);
        } else {
            ensure!(paths.items.iter().any(|s|s.id==id), "Path not found");
            paths.items.retain(|s|s.id!=id);
        }
        paths.validate(frame)?;
        std::fs::create_dir_all(&self.directory)?;
        let temporary=self.directory.join(format!(".{}.tmp",uuid::Uuid::new_v4()));
        let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(&serde_json::to_vec(&paths)?)?;file.sync_all()?;
        std::fs::rename(temporary,self.path(scene)?)?;
        Ok(paths)
    }
}
pub struct Publisher {
    _component: Component,
    output: ConfiguredObservable<Paths>,
    capture: BufferProductCapture<Paths>,
    last: Option<Paths>,
}
impl Publisher {
    pub fn mount(runtime:&ComponentRuntime, endpoint:&auki_component_protocol::ComponentProtocolEndpoint,
        id:&str, owner:&str, frame:&str, clock:ClockReference)->Result<Self> {
        let component=runtime.component(ComponentSpec::new(format!("scene-paths:{id}")).observable(ObservableContract {
            name:"paths".into(),datatype:SCHEMA.into(),schema:SCHEMA.into(),access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster }))?;
        let output=component.configured_observable(ConfiguredObservableSpec::new("paths",uuid::Uuid::new_v4().to_string(),clock,
            PayloadContract::Structured(StructuredPayloadContract {modality:"scenegraph".into(),datatype:SCHEMA.into(),schema:SCHEMA.into(),observes:"authored_surface_paths".into(),unit:Some("meters".into())}))
            .in_registered_frame(crate::conventions::registry(owner,frame)?))?;
        component.expose()?;
        let capture=runtime.capture_buffer(format!("local-scene:{id}:paths"),&output,BufferLimits::entries(1),|v:&Paths|serde_json::to_vec(v).map_or(usize::MAX,|b|b.len()))?;
        endpoint.export_product(&capture.product())?;
        Ok(Self {_component:component,output,capture,last:None})
    }
    pub fn reference(&self)->ProductReference { self.capture.product().reference() }
    pub fn publish(&mut self, timestamp:u64, paths:Paths)->Result<()> {
        paths.validate(&paths.frame)?;
        if self.last.as_ref()!=Some(&paths) {
            self.output.publish(timestamp,Arc::new(paths.clone()))?;
            ensure!(self.capture.errors().is_empty(),"Path Product retention failed");
            self.last=Some(paths);
        }
        Ok(())
    }
}
