//! Bounded measured depth Product; no local map and no network outside SDK.
use anyhow::{Result, ensure};
use auki_components::*;
use auki_protocols::registry::v3::RegistryKind;
use serde::Deserialize;
use crate::depth_points::{Points, PointsContract, POINTS_SCHEMA, POINTS_METADATA};

pub const PRODUCT: &str = "bbos-depth-points";
const EXTRINSIC: &str = "bbos-depth-extrinsic";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration { pub sensor_to_base: auki_odometry::Pose }
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture { pub timestamp_ns: u64, pub points: Vec<[f64;3]>, pub colors: Option<Vec<[u8;3]>> }

pub fn start() -> (tokio::task::JoinHandle<()>, tokio::sync::watch::Receiver<Option<(Configuration,Capture)>>) {
    let (tx,rx) = tokio::sync::watch::channel(None);
    let task=tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let work=async {
            let mut child=tokio::process::Command::new("python3")
                .args(["scripts/bracketbot-python.py","scripts/diagnostic-depth.py"])
                .env_remove("AUKI_ACCESS_KEY").env_remove("AUKI_APP_SECRET")
                .env_remove("AUKI_EMAIL").env_remove("AUKI_PASSWORD")
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped())
                .kill_on_drop(true).spawn()?;
            let mut lines=tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
            let first=tokio::time::timeout(std::time::Duration::from_secs(30),lines.next_line()).await??
                .ok_or_else(||anyhow::anyhow!("Depth configuration missing"))?;
            let config:Configuration=serde_json::from_str(&first)?;
            config.sensor_to_base.validate()?;
            loop {
                let line=lines.next_line().await?.ok_or_else(||anyhow::anyhow!("Depth capture ended"))?;
                ensure!(line.len()<=256*1024,"Depth IPC sample too large");
                let sample:Capture=serde_json::from_str(&line)?;
                tx.send_replace(Some((config.clone(),sample)));
            }
            #[allow(unreachable_code)] Ok::<(),anyhow::Error>(())
        };
        if let Err(e)=work.await {eprintln!("Depth capture failed: {e}");}
    });
    (task,rx)
}

pub struct Depth {
    output: ConfiguredObservable<Points>,
    capture: BufferProductCapture<Points>,
    mount: BufferProductCapture<crate::compact_pose::Update>,
    _components: Vec<Component>,
    last: u64,
}
impl Depth {
    pub fn mount(p: &crate::poses::Publisher, config: Configuration) -> Result<Self> {
        config.sensor_to_base.validate()?;
        let frame = auki_registry::FrameRegistryEntry::ros_optical(&p.contract.from_frame.peer_id,
            format!("bbos-depth-optical-{}",p.contract.session_id));
        p.definitions.lock().map_err(|_|anyhow::anyhow!("Registry unavailable"))?.push((
            RegistryKind::Frame,frame.frame_id.clone(),frame.hash(),String::from_utf8(frame.canonical_bytes())?));
        let c = PointsContract {frame:(&frame).into(),clock:p.contract.clock.clone(),session_id:p.contract.session_id.clone()};
        let component = p.runtime.component(ComponentSpec::new("bbos-depth").observable(ObservableContract {
            name:"points".into(),datatype:POINTS_SCHEMA.into(),schema:POINTS_SCHEMA.into(),
            access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster }))?;
        let output = component.configured_observable(ConfiguredObservableSpec::new("points",c.output_id(),c.clock.clone(),
            PayloadContract::Structured(StructuredPayloadContract {datatype:POINTS_SCHEMA.into(),schema:POINTS_SCHEMA.into(),
                modality:"depth".into(),observes:"measured_depth_points".into(),unit:Some("meters".into())}))
            .in_registered_frame(frame.clone()))?;
        component.expose()?;
        let capture = p.runtime.capture_buffer_with_metadata(PRODUCT,&output,
            BufferLimits {max_entries:Some(4),max_bytes:Some(1024*1024),target_duration:None},
            |v:&Points|v.points.len()*32,move |o|Ok(Some(CatalogProductMetadata {schema:POINTS_METADATA.into(),
                source_sequence:o.sequence,value:serde_json::to_value(&c).map_err(|e|e.to_string())?})))?;
        let transform = crate::compact_pose::Contract {from_frame:(&frame).into(),to_frame:(&p.contract.from_frame).into(),
            clock:p.contract.clock.clone(),session_id:p.contract.session_id.clone(),lifetime:crate::compact_pose::Lifetime::Held};
        let mount_component = p.runtime.component(ComponentSpec::new(EXTRINSIC).observable(ObservableContract {
            name:"transform".into(),datatype:crate::compact_pose::SCHEMA.into(),schema:crate::compact_pose::SCHEMA.into(),
            access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster }))?;
        let mount_output = mount_component.configured_observable(ConfiguredObservableSpec::new("transform",transform.output_id(),transform.clock.clone(),
            PayloadContract::Structured(StructuredPayloadContract {datatype:crate::compact_pose::SCHEMA.into(),schema:crate::compact_pose::SCHEMA.into(),
                modality:"pose".into(),observes:"depth_sensor_to_base".into(),unit:None})).in_registered_frame(p.contract.from_frame.clone()))?;
        mount_component.expose()?;
        let mount = p.runtime.capture_buffer_with_metadata(EXTRINSIC,&mount_output,
            BufferLimits {max_entries:Some(1),max_bytes:Some(4096),target_duration:None},|_:&crate::compact_pose::Update|256,
            move |o|Ok(Some(CatalogProductMetadata {schema:crate::compact_pose::METADATA.into(),source_sequence:o.sequence,
                value:serde_json::to_value(&transform).map_err(|e|e.to_string())?})))?;
        mount_output.publish(0,std::sync::Arc::new(crate::compact_pose::Update {
            tracking:auki_odometry::Tracking::Valid {pose:config.sensor_to_base}}))?;
        p.endpoint.export_product(&mount.product())?;
        p.endpoint.export_product(&capture.product())?;
        Ok(Self {output,capture,mount,_components:vec![component,mount_component],last:0})
    }
    pub fn publish(&mut self, sample: Capture) -> Result<()> {
        ensure!(sample.timestamp_ns>self.last,"Depth capture timestamp did not advance");
        let points = Points {points:sample.points,colors:sample.colors}; points.validate()?;
        self.output.publish(sample.timestamp_ns,std::sync::Arc::new(points))?;
        ensure!(self.capture.errors().is_empty(),"Depth capture retention failed");
        self.last=sample.timestamp_ns;
        Ok(())
    }
    pub fn close(self,p:&crate::poses::Publisher)->Result<()> {
        p.endpoint.unexport_product(PRODUCT); p.endpoint.unexport_product(EXTRINSIC);
        self.capture.delete()?; self.mount.delete()?; Ok(())
    }
}
