//! Home-authored polylines: metres in an explicit scene frame, never camera pixels.
use anyhow::{Result, ensure, anyhow};
use auki_components::*;
use serde::{Serialize, Deserialize};
use std::{path::PathBuf, sync::{Arc, Mutex}, collections::BTreeSet};
pub const SCHEMA: &str = "spatial-harness.scene-paths/v1";
pub const COMPONENT: &str = "scene-paths";

/// A peer adds a path it drew. `frame` is the scene frame the points are in;
/// Home refuses a path drawn for another frame.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Add { pub frame: String, pub stroke: Stroke }
impl ContractType for Add { const DATATYPE: &'static str = "spatial-harness.scene-path-add/v1"; }
/// A peer removes a saved path by its ID.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Remove { pub frame: String, pub id: String }
impl ContractType for Remove { const DATATYPE: &'static str = "spatial-harness.scene-path-remove/v1"; }
/// What a phone's app hands its core: one of the two requests above.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Change { Add(Add), Remove(Remove) }

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
/// Send one path change to Home's path component for the paired scene.
pub async fn send(peer: &auki_sdk::AukiPeer, home: &crate::network::HomeConnection, change: &Change) -> Result<bool> {
    use auki_component_protocol::{CatalogResponse, ComponentProtocolClient};
    use std::time::Duration;
    let expected = home.validate(peer)?;
    let client = ComponentProtocolClient::new(peer.protocols());
    for route in &home.routes {
        let route: auki_sdk::Multiaddr = route.parse()?;
        let Ok(Ok(CatalogResponse::Snapshot { snapshot })) = tokio::time::timeout(
            Duration::from_secs(1), client.catalog_exact(expected, route.clone(), None)).await else { continue; };
        let component = snapshot.components.iter()
            .find(|c| c.manifest.component_id == home.scoped_component(COMPONENT))
            .ok_or_else(|| anyhow!("Home path component unavailable"))?.manifest.reference();
        let (request, wait) = (uuid::Uuid::new_v4().to_string(), Some(Duration::from_secs(3)));
        return Ok(match change {
            Change::Add(add) => client.invoke_exact::<_, bool>(expected, route, component, "add", "phone-path", request, wait, add).await?,
            Change::Remove(remove) => client.invoke_exact::<_, bool>(expected, route, component, "remove", "phone-path", request, wait, remove).await?,
        }.result);
    }
    anyhow::bail!("Home path component unreachable")
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
        self.write(scene,frame,id,stroke,false)
    }
    /// Extend a live stroke without allowing delayed updates to recreate a deleted path.
    pub fn extend(&self, scene: &str, frame: &str, stroke: Stroke) -> Result<Paths> {
        let id=stroke.id.clone();self.write(scene,frame,&id,Some(stroke),true)
    }
    fn write(&self, scene: &str, frame: &str, id: &str, stroke: Option<Stroke>, extend:bool) -> Result<Paths> {
        use std::io::Write;
        let _guard=self.writer.lock().map_err(|_|anyhow!("Path storage unavailable"))?;
        let mut paths=self.get(scene,frame)?;
        if let Some(stroke)=stroke {
            ensure!(stroke.id==id, "Path identity mismatch");
            if let Some(existing)=paths.items.iter_mut().find(|s|s.id==id) {
                if existing==&stroke { return Ok(paths); }
                ensure!(extend,"Path already exists with different content");
                ensure!(existing.title==stroke.title && existing.color==stroke.color && existing.width_m==stroke.width_m,"Live path metadata changed");
                ensure!(stroke.segments.len()>=existing.segments.len() && existing.segments.iter().enumerate().all(|(i,s)| {
                    if i+1<existing.segments.len() {stroke.segments[i]==*s} else {stroke.segments[i].starts_with(s)}
                }),"Live path updates must append points");
                *existing=stroke;
            } else {
                ensure!(!extend,"Path no longer exists");
                paths.items.push(stroke);
            }
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
    /// `store` adds a path (`Some`) or removes one (`None`) in this scene's saved paths.
    pub fn mount(runtime:&ComponentRuntime, endpoint:&auki_component_protocol::ComponentProtocolEndpoint,
        id:&str, owner:&str, frame:&str, clock:ClockReference,
        store: impl Fn(&str, Option<Stroke>) -> Result<Paths> + Send + Sync + 'static)->Result<Self> {
        let operable=|name:&str,instruction:&str| OperableContract { name:name.into(), instruction:instruction.into(), result:"bool".into(), exposure:Exposure::Cluster };
        let component=runtime.component(ComponentSpec::new(format!("{COMPONENT}:{id}")).observable(ObservableContract {
            name:"paths".into(),datatype:SCHEMA.into(),schema:SCHEMA.into(),access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster })
            .operable(operable("add",Add::DATATYPE)).operable(operable("remove",Remove::DATATYPE)))?;
        // Any authenticated peer of the scene may draw or erase, as any browser on Home's page may.
        let store=Arc::new(store);
        let (adding,scene)=(store.clone(),frame.to_owned());
        let add=component.operable("add",|ctx|!ctx.caller_peer_id.is_empty(),move |_,request:Add| {
            if request.frame!=scene { return Err(InvocationError::Rejected("Scene frame changed; redraw the path".into())); }
            let id=request.stroke.id.clone();
            adding(&id,Some(request.stroke)).map(|_|true).map_err(|e|InvocationError::Rejected(e.to_string()))
        })?;
        let scene=frame.to_owned();
        let remove=component.operable("remove",|ctx|!ctx.caller_peer_id.is_empty(),move |_,request:Remove| {
            if request.frame!=scene { return Err(InvocationError::Rejected("Scene frame changed".into())); }
            store(&request.id,None).map(|_|true).map_err(|e|InvocationError::Rejected(e.to_string()))
        })?;
        let output=component.configured_observable(ConfiguredObservableSpec::new("paths",uuid::Uuid::new_v4().to_string(),clock,
            PayloadContract::Structured(StructuredPayloadContract {modality:"scenegraph".into(),datatype:SCHEMA.into(),schema:SCHEMA.into(),observes:"authored_surface_paths".into(),unit:Some("meters".into())}))
            .in_registered_frame(crate::conventions::registry(owner,frame)?))?;
        component.expose()?;
        endpoint.export_operable(&add)?;
        endpoint.export_operable(&remove)?;
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
