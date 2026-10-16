//! Bounded measured depth Product; no local map and no network outside SDK.
use anyhow::{Result, ensure};
use auki_components::*;
use auki_protocols::registry::v3::RegistryKind;
use serde::Deserialize;
use crate::component_enabled::{SetEnabled, State, OPERATION, STATE_SCHEMA};
use std::sync::{Arc, Mutex};
use crate::depth_points::{Points, PointsContract, PackedPoints, PACKED_POINTS_SCHEMA, POINTS_METADATA};

pub const PRODUCT: &str = "bbos-depth-points";
const EXTRINSIC: &str = "bbos-depth-extrinsic";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration { pub sensor_to_base: auki_odometry::Pose }
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture { pub timestamp_ns: u64, pub points: Vec<[f64;3]>, pub colors: Option<Vec<[u8;3]>> }

pub fn start() -> (tokio::task::JoinHandle<()>, tokio::sync::watch::Receiver<Option<(Configuration,Option<Capture>)>>,tokio::sync::watch::Sender<bool>) {
    let (tx,rx) = tokio::sync::watch::channel(None);
    let (demand_tx,mut demand_rx)=tokio::sync::watch::channel(false);
    let task=tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt,AsyncReadExt,AsyncWriteExt};
        let work=async {
            let mut child=tokio::process::Command::new("python3")
                .args(["scripts/bracketbot-python.py","scripts/diagnostic-depth.py"])
                .env_remove("AUKI_ACCESS_KEY").env_remove("AUKI_APP_SECRET")
                .env_remove("AUKI_EMAIL").env_remove("AUKI_PASSWORD")
                .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
                .kill_on_drop(true).spawn()?;
            let mut reader=tokio::io::BufReader::new(child.stdout.take().unwrap());
            let mut first=String::new();
            let bytes=tokio::time::timeout(std::time::Duration::from_secs(30),reader.read_line(&mut first)).await??;
            ensure!(bytes>0 && bytes<=16384,"Depth configuration missing or too large");
            let config:Configuration=serde_json::from_str(&first)?;
            config.sensor_to_base.validate()?;
            // Make the empty Product discoverable before requesting any capture.
            tx.send_replace(Some((config.clone(),None)));
            let mut stdin=child.stdin.take().unwrap();
            let controls=async {
                loop {
                    demand_rx.changed().await?;
                    let on=*demand_rx.borrow_and_update();
                    stdin.write_all(if on {b"1\n"} else {b"0\n"}).await?;
                }
                #[allow(unreachable_code)] Ok::<(),anyhow::Error>(())
            };
            let captures=async {
                loop {
                    // Local IPC only: capture ns, count, RGB flag, f32 XYZ, u8 RGB.
                    // This reader is never cancelled/restarted mid-record by demand changes.
                    let timestamp_ns=reader.read_u64_le().await?;
                    let count=reader.read_u32_le().await? as usize;
                    let colors=reader.read_u8().await?;
                    ensure!(count>0 && count<=crate::depth_points::MAX_POINTS && colors<=1,"Invalid depth IPC header");
                    let mut data=vec![0;count*12];reader.read_exact(&mut data).await?;
                    let points=data.chunks_exact(12).map(|p|std::array::from_fn(|i|
                        f32::from_le_bytes(p[i*4..i*4+4].try_into().unwrap()) as f64)).collect();
                    let colors=if colors==1 {let mut data=vec![0;count*3];reader.read_exact(&mut data).await?;
                        Some(data.chunks_exact(3).map(|c|[c[0],c[1],c[2]]).collect())} else {None};
                    tx.send_replace(Some((config.clone(),Some(Capture {timestamp_ns,points,colors}))));
                }
                #[allow(unreachable_code)] Ok::<(),anyhow::Error>(())
            };
            tokio::select! {r=controls=>r,r=captures=>r}
        };
        if let Err(e)=work.await {crate::diagnostic_log!("Depth capture failed: {e}");}
    });
    (task,rx,demand_tx)
}

pub struct Depth {
    output: ConfiguredObservable<PackedPoints>,
    capture: BufferProductCapture<PackedPoints>,
    contract: PointsContract,
    mount: BufferProductCapture<crate::compact_pose::Update>,
    _components: Vec<Component>,
    last: u64,
    last_native: u64,
    enabled: Arc<Mutex<bool>>,
    state: BufferProductCapture<State>,
    _set_enabled: Operable<SetEnabled, State>,
    native_clock: crate::capture_clock::NativeCaptureClock,
}
impl Depth {
    pub fn mount(p: &crate::poses::Publisher, config: Configuration, controller: String) -> Result<Self> {
        ensure!(!controller.is_empty(), "Missing authorized controller");
        config.sensor_to_base.validate()?;
        let frame = auki_registry::FrameRegistryEntry::ros_optical(&p.contract.from_frame.peer_id,
            format!("bbos-depth-optical-{}",p.contract.session_id));
        p.definitions.lock().map_err(|_|anyhow::anyhow!("Registry unavailable"))?.push((
            RegistryKind::Frame,frame.frame_id.clone(),frame.hash(),String::from_utf8(frame.canonical_bytes())?));
        let c = PointsContract {frame:(&frame).into(),clock:p.contract.clock.clone(),session_id:p.contract.session_id.clone()};
        let component = p.runtime.component(ComponentSpec::new("bbos-depth").observable(ObservableContract {
            name:"points".into(),datatype:PACKED_POINTS_SCHEMA.into(),schema:PACKED_POINTS_SCHEMA.into(),
            access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster })
            .observable(ObservableContract {name:"enabled".into(),datatype:STATE_SCHEMA.into(),schema:STATE_SCHEMA.into(),
                access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster})
            .operable(OperableContract {name:OPERATION.into(),instruction:SetEnabled::DATATYPE.into(),
                result:State::DATATYPE.into(),exposure:Exposure::Cluster}))?;
        let enabled=Arc::new(Mutex::new(true));
        let state_output=component.configured_observable(ConfiguredObservableSpec::new("enabled",
            format!("depth-enabled-{}",p.contract.session_id),p.contract.clock.clone(),
            PayloadContract::Structured(StructuredPayloadContract {datatype:STATE_SCHEMA.into(),schema:STATE_SCHEMA.into(),
                modality:"state".into(),observes:"publication_enabled".into(),unit:None})))?;
        let operation_clock=p.session_clock.clone();
        let gate=enabled.clone();
        let operation_output=state_output.clone();
        let set_enabled=component.operable(OPERATION,move |ctx|ctx.caller_peer_id==controller,
            move |_,request:SetEnabled|->std::result::Result<State,InvocationError>{
                let mut gate=gate.lock().map_err(|_|InvocationError::TargetUnavailable)?;
                let result=State {enabled:request.enabled};
                operation_output.publish(operation_clock.now_ns(),Arc::new(result.clone())).map_err(|e|InvocationError::Rejected(e.to_string()))?;
                *gate=request.enabled;
                Ok(result)
            })?;
        let output = component.configured_observable(ConfiguredObservableSpec::new("points",c.output_id(),c.clock.clone(),
            PayloadContract::Structured(StructuredPayloadContract {datatype:PACKED_POINTS_SCHEMA.into(),schema:PACKED_POINTS_SCHEMA.into(),
                modality:"depth".into(),observes:"measured_depth_points".into(),unit:Some("meters".into())}))
            .in_registered_frame(frame.clone()))?;
        component.expose()?;
        let state=p.runtime.capture_buffer_with_metadata("bbos-depth-enabled",&state_output,
            BufferLimits {max_entries:Some(1),max_bytes:Some(4096),target_duration:None},|_:&State|64,
            |o|Ok(Some(CatalogProductMetadata {schema:STATE_SCHEMA.into(),source_sequence:o.sequence,
                value:serde_json::to_value(&*o.payload).map_err(|e|e.to_string())?})))?;
        state_output.publish(p.session_clock.now_ns(),Arc::new(State {enabled:true}))?;
        let capture = p.runtime.capture_buffer_with_metadata(PRODUCT,&output,
            BufferLimits {max_entries:Some(1),max_bytes:Some(128*1024),target_duration:None},
            |v:&PackedPoints|v.as_bytes().len(),move |o|Ok(Some(CatalogProductMetadata {schema:POINTS_METADATA.into(),
                source_sequence:o.sequence,value:serde_json::to_value(o.payload.contract()).map_err(|e|e.to_string())?})))?;
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
        p.endpoint.export_product_with_encoder(&capture.product(),PackedPoints::encoder()?)?;
        p.endpoint.export_product(&state.product())?;
        p.endpoint.export_operable(&set_enabled)?;
        Ok(Self {output,capture,mount,_components:vec![component,mount_component],last:0,last_native:0,enabled,state,_set_enabled:set_enabled,contract:c,
            native_clock:p.native_clock.clone()})
    }
    pub fn wants_capture(&self,endpoint:&auki_component_protocol::ComponentProtocolEndpoint)->bool {
        self.enabled.lock().is_ok_and(|enabled|*enabled) && endpoint.product_active_subscriptions(PRODUCT)>0
    }
    pub fn publish(&mut self, sample: Capture,endpoint:&auki_component_protocol::ComponentProtocolEndpoint) -> Result<()> {
        // Serialize with the operation: after disable is acknowledged no new
        // sample can enter the Product. The BBOS capture process keeps running.
        let enabled=self.enabled.lock().map_err(|_|anyhow::anyhow!("Depth control unavailable"))?;
        if !*enabled || endpoint.product_active_subscriptions(PRODUCT)==0 {return Ok(());}
        ensure!(sample.timestamp_ns>self.last_native,"Depth native capture timestamp did not advance");
        let Some(timestamp_ns)=self.native_clock.convert(sample.timestamp_ns)? else {return Ok(());};
        ensure!(timestamp_ns>self.last,"Depth capture timestamp did not advance");
        let points = Points {points:sample.points,colors:sample.colors}; points.validate()?;
        let packed=PackedPoints::from_points(self.contract.clone(),&points)?;
        self.output.publish(timestamp_ns,std::sync::Arc::new(packed))?;
        ensure!(self.capture.errors().is_empty(),"Depth capture retention failed");
        self.last=timestamp_ns;
        self.last_native=sample.timestamp_ns;
        Ok(())
    }
    pub fn close(self,p:&crate::poses::Publisher)->Result<()> {
        p.endpoint.unexport_product(PRODUCT); p.endpoint.unexport_product(EXTRINSIC);
        p.endpoint.unexport_operable("bbos-depth",OPERATION);
        p.endpoint.unexport_product("bbos-depth-enabled");
        self.capture.delete()?; self.mount.delete()?; self.state.delete()?; Ok(())
    }
}
