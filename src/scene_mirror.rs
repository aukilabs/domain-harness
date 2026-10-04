//! A remote scene is a separate view, never merged into Home's canonical map.
use crate::network::{CanonicalScene, HomeConnection, subscribe_scene};
use anyhow::{Result, anyhow, bail, ensure};
use auki_component_protocol::RemoteObservationEvent;
use auki_scenegraph::MapSnapshot;
use auki_sdk::AukiPeer;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
#[derive(Clone, Default)]
pub struct RemoteScene {
    pub scene: Option<CanonicalScene>,
    pub status: String,
    pub detail: String,
    pub(crate) received: Option<Instant>,
}
impl RemoteScene {
    pub fn status_view(&self) -> Value {
        let status = if self.status == "connected"
            && self
                .received
                .is_some_and(|t| t.elapsed() > Duration::from_secs(10))
        {
            "stale"
        } else {
            &self.status
        };
        json!({"status":status,"detail":self.detail,"available":self.scene.is_some(),"age_ms":self.received.map(|t|t.elapsed().as_millis() as u64),"peer_id":self.scene.as_ref().map(|s|&s.authority_peer_id)})
    }
    pub fn render(&self) -> Result<Value> {
        let s = self
            .scene
            .as_ref()
            .ok_or_else(|| anyhow!("Runner scene has not arrived yet"))?;
        let mut view = render_scene(s)?;
        view["remote_status"] = self.status_view();
        Ok(view)
    }
}
pub fn render_scene(s: &CanonicalScene) -> Result<Value> {
    let local = crate::local_mapper::is_local_frame(&s.frame);
    let display = if local {
        crate::local_mapper::display_frame(&s.frame)?
    } else {
        crate::model::FRAME.into()
    };
    let target = crate::conventions::registry("home", &display)?;
    let portals = crate::conventions::portals(&s.portals, &display)?;
    let notes = crate::notes::from_registries(&s.registries, &s.frame)?
        .in_frame(&s.voxels.definition.frame, &target)?;
    let mut voxels = crate::model::occupied(&s.voxels)?;
    for v in &mut voxels {
        v.center = crate::conventions::point(v.center, &s.voxels.definition.frame, &target)?;
    }
    let mut objects = s.objects.clone();
    for o in &mut objects {
        o.position = crate::conventions::point(o.position, &s.voxels.definition.frame, &target)?;
        o.ray_origin =
            crate::conventions::point(o.ray_origin, &s.voxels.definition.frame, &target)?;
        o.hit.position =
            crate::conventions::point(o.hit.position, &s.voxels.definition.frame, &target)?;
        o.hit.center =
            crate::conventions::point(o.hit.center, &s.voxels.definition.frame, &target)?;
    }
    let mut metadata = s
        .registries
        .get("local_mapping")
        .cloned()
        .unwrap_or(Value::Null);
    if let Some(robot) = metadata.get_mut("robot_pose").filter(|v| !v.is_null()) {
        let pose = serde_json::from_value(robot["pose"].clone())?;
        robot["pose"] = serde_json::to_value(crate::conventions::pose(
            &pose,
            &s.voxels.definition.frame,
            &target,
        )?)?;
        robot["frame"] = json!(display);
    }
    if let Some(runners) = metadata.get_mut("runners").and_then(Value::as_object_mut) {
        for runner in runners.values_mut() {
            if let Some(robot) = runner.get_mut("robot_pose").filter(|v| !v.is_null()) {
                let pose = serde_json::from_value(robot["pose"].clone())?;
                robot["pose"] = serde_json::to_value(crate::conventions::pose(
                    &pose,
                    &s.voxels.definition.frame,
                    &target,
                )?)?;
                robot["frame"] = json!(display);
            }
        }
    }
    let mut view = json!({"revision":s.revision,"frame":display,"local_map":local,"local_mapping":metadata,"source_frame":s.frame,"source_convention":s.portals.scenegraph.map.frame.convention,"convention":"OpenGL · Y up · meters",
            "voxel_size":s.voxels.definition.grid().voxel_size_m.0,"voxels":voxels,
            "portals":portals.scenegraph.anchors.values().collect::<Vec<_>>(),"objects":objects,"notes":notes.items,
            "sources":{"galbot":s.voxels.integrated_observations},"integrated_observations":s.voxels.integrated_observations,
            "sdk_revision":s.sdk_revision,"mode":"Runner-owned scenegraph","demo":false,"synthetic_layout":null,"sensor_positions":{},
            "authority":"Galbot mapper","authority_peer_id":s.authority_peer_id,"remote_status":null});
    crate::mobile_pose::append_labels(&mut view, &s.registries["mobile_presence"]);
    Ok(view)
}

pub async fn monitor(
    peer: &AukiPeer,
    config: Option<&HomeConnection>,
    portals: &MapSnapshot,
    state: &watch::Sender<RemoteScene>,
    stop: &CancellationToken,
) {
    let Some(config) = config else {
        state.send_modify(|s| s.status = "not configured".into());
        stop.cancelled().await;
        return;
    };
    loop {
        state.send_modify(|s| {
            s.status = "connecting".into();
            s.detail = "Connecting to Galbot’s scene publisher".into();
        });
        let work = async {
            let mut subscription = subscribe_scene(peer, config).await?;
            let result=async {
                let mut previous=None;
                let mut previous_frame=None;
                loop {
                    let event=tokio::select! {_=stop.cancelled()=>return Ok(()),event=subscription.next()=>event?};
                    match event {
                        Some(RemoteObservationEvent::Observation(o))=>{
                            let s=&o.payload;s.validate(&config.peer_id)?;
                            ensure!(s.frame==config.frame || (config.frame==crate::local_mapper::FAMILY && crate::local_mapper::is_local_frame(&s.frame)),"runner frame differs from connection profile");
                            ensure!(crate::model::clock_ref(&s.publication_clock)==subscription.product().producer.clock,"scene clock mismatch");
                            if !crate::local_mapper::is_local_frame(&s.frame) {ensure!(crate::conventions::matching_portals(&s.portals,portals)?,"Runner scene portal reference differs from Home");}
                            ensure!(previous_frame.as_ref()!=Some(&s.frame) || previous.is_none_or(|r|s.revision>r),"runner scene revision did not advance");
                            previous=Some(s.revision);previous_frame=Some(s.frame.clone());
                            state.send_replace(RemoteScene{scene:Some((**s).clone()),status:"connected".into(),detail:if crate::local_mapper::is_local_frame(&s.frame){if let Some(anchor)=s.registries["local_mapping"]["alignment_to_home"]["anchor"].as_str(){format!("Robot boot origin · blue observations · red Home references aligned via {anchor}")}else{"Robot boot origin · waiting for shared portal alignment".into()}}else{format!("Galbot {} → Home OpenGL · SDK conversion",s.portals.scenegraph.map.frame.convention.map(|c|c.as_str()).unwrap_or("unknown"))},received:Some(Instant::now())});
                        },
                        Some(RemoteObservationEvent::Gap(_))=>{}, // Complete snapshots replace prior state.
                        _=>bail!("Runner scene stream ended"),
                    }
                }
            }.await;
            let cleanup = subscription.close().await;
            result.and(cleanup.map_err(Into::into))
        };
        let result = tokio::select! {_=stop.cancelled()=>break,result=work=>result};
        if let Err(e) = result {
            state.send_modify(|s| {
                s.status = "disconnected".into();
                s.detail = e.to_string();
            });
        }
        tokio::select! {_=stop.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(5))=>{}}
    }
    state.send_modify(|s| s.status = "stopped".into());
}
