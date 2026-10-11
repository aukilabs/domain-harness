//! A remote scene is a separate view, never merged into Home's canonical map.
use crate::network::{CanonicalScene, HomeConnection, subscribe_scene};
use anyhow::{Result, anyhow, bail, ensure};
use auki_component_protocol::RemoteObservationEvent;
use auki_registry::FrameRegistryEntry;
use auki_scenegraph::MapSnapshot;
use auki_sdk::AukiPeer;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

async fn before_deadline<T>(deadline: tokio::time::Instant,
    work: impl std::future::Future<Output=Result<T>>) -> Result<T> {
    tokio::time::timeout_at(deadline,work).await
        .map_err(|_|anyhow!("Runner scene stalled; rediscovering current publisher"))?
}
#[derive(Clone, Default)]
pub struct RemoteScene {
    pub scene: Option<CanonicalScene>,
    pub status: String,
    pub detail: String,
    pub(crate) received: Option<Instant>,
    pub frame_transform: Option<crate::frame_transform::Received>,
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
        if let Some(transform) = &self.frame_transform {
            let usable = self.status_view()["status"] == "connected" && transform.usable(s);
            view["frame_transform"] = serde_json::to_value(&transform.value)?;
            if usable {
                apply_display_transform(&mut view, s, &transform.value)?;
            } else if transform.value.validity == crate::frame_transform::Validity::Valid {
                view["frame_transform"]["validity"] = json!("stale");
                view["frame_transform"]["reason"] = json!("Transform stream stale or does not match current scene evidence");
            }
        }
        view["remote_status"] = self.status_view();
        Ok(view)
    }
}
pub fn render_scene(s: &CanonicalScene) -> Result<Value> {
    let local = crate::local_mapper::is_local_frame(&s.frame);
    let display = if crate::local_scenes::frame_scene_id(&s.frame).is_some() {
        s.frame.clone()
    } else if local {
        crate::local_mapper::display_frame(&s.frame)?
    } else {
        crate::model::FRAME.into()
    };
    let target = crate::conventions::registry("home", &display)?;
    let portals = crate::conventions::portals(&s.portals, &display)?;
    let landmarks =
        crate::landmarks::from_registries(&s.registries, &s.frame)?.in_frame(&display)?;
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
        render_robot_pose(robot, s, &target)?;
    }
    if let Some(runners) = metadata.get_mut("runners").and_then(Value::as_object_mut) {
        for runner in runners.values_mut() {
            if let Some(robot) = runner.get_mut("robot_pose").filter(|v| !v.is_null()) {
                render_robot_pose(robot, s, &target)?;
            }
        }
    }
    let mut view = json!({"frame":display,"local_map":local,"local_mapping":metadata,"source_frame":s.frame,"source_convention":s.portals.scenegraph.map.frame.convention,"convention":"OpenGL · Y up · meters",
            "voxel_size":s.voxels.definition.grid().voxel_size_m.0,"voxels":voxels,
            "portals":portals.scenegraph.anchors.values().collect::<Vec<_>>(),"objects":objects,"notes":notes.items,"volumes":notes.volumes,
            "sources":{"galbot":s.voxels.integrated_observations},"integrated_observations":s.voxels.integrated_observations,
            "sdk_revision":s.sdk_revision,"mode":"Runner-owned scenegraph","demo":false,"synthetic_layout":null,"sensor_positions":{},
            "authority":"Galbot mapper","authority_peer_id":s.authority_peer_id,"remote_status":null});
    crate::mobile_pose::append_labels(&mut view, &s.registries["mobile_presence"]);
    view["landmarks"] = serde_json::to_value(landmarks)?;
    Ok(view)
}

/// Apply a separately received rigid relationship to the presentation only.
/// Neither the native voxel grid nor any robot-owned snapshot is rewritten.
pub fn display_transform(scene: &CanonicalScene, value: &crate::frame_transform::FrameTransform, display: &str) -> Result<(String, auki_odometry::Pose)> {
    use crate::local_mapper::{compose, from_rigid, inverse, rigid, identity};
    value.validate(&scene.authority_peer_id)?;
    ensure!(value.validity == crate::frame_transform::Validity::Valid && value.source_frame == scene.portals.scenegraph.map.frame, "Transform is not valid for this robot frame");
    let published = value.transform.as_ref().ok_or_else(||anyhow!("Missing transform"))?;
    let target_display = if crate::local_mapper::is_local_frame(&published.to_frame_id) {
        crate::local_mapper::display_frame(&published.to_frame_id)?
    } else if crate::conventions::convention(&published.to_frame_id)? == auki_registry::CoordinateConvention::OpenGl {
        published.to_frame_id.clone()
    } else {
        crate::model::FRAME.into()
    };
    let target_axes = crate::conventions::pose(&identity(),
        &crate::conventions::registry("home", &published.to_frame_id)?,
        &crate::conventions::registry("home", &target_display)?)?;
    let transform = rigid(&compose(&target_axes, &from_rigid(published))?, &published.from_frame_id, &target_display);
    let display_frame = crate::conventions::registry("home", display)?;
    let native_to_display = crate::conventions::pose(&identity(), &scene.voxels.definition.frame, &display_frame)?;
    let display_to_scene = compose(&from_rigid(&transform), &inverse(&native_to_display)?)?;
    Ok((target_display, display_to_scene))
}

pub fn apply_display_transform(view: &mut Value, scene: &CanonicalScene, value: &crate::frame_transform::FrameTransform) -> Result<()> {
    use crate::local_mapper::{compose, from_rigid, rigid, identity};
    let display = view["frame"].as_str().ok_or_else(||anyhow!("Missing display frame"))?;
    let display_frame = crate::conventions::registry("home", display)?;
    let (target_display, display_to_scene) = display_transform(scene, value, display)?;
    let transform = rigid(&display_to_scene, display, &target_display);
    let point = |v: &mut Value| -> Result<()> {
        let translation: [f64;3] = serde_json::from_value(v.clone())?;
        *v = json!(compose(&display_to_scene, &auki_odometry::Pose{translation,..identity()})?.translation);
        Ok(())
    };
    let anchor = |v: &mut Value| -> Result<()> {
        let p: auki_scenegraph::RigidTransform = serde_json::from_value(v.clone())?;
        *v=serde_json::to_value(rigid(&compose(&display_to_scene,&from_rigid(&p))?, &p.from_frame_id,&transform.to_frame_id))?;
        Ok(())
    };
    for v in view["voxels"].as_array_mut().into_iter().flatten(){point(&mut v["center"])?;}
    for v in view["portals"].as_array_mut().into_iter().flatten(){anchor(&mut v["pose_in_map"])?;}
    for v in view["objects"].as_array_mut().into_iter().flatten(){
        point(&mut v["position"])?; point(&mut v["ray_origin"])?;
        point(&mut v["hit"]["position"])?;point(&mut v["hit"]["center"])?;
    }
    for v in view["notes"].as_array_mut().into_iter().flatten(){
        point(&mut v["position"])?;
        if v["anchor_surface"]["frame"].as_str()==Some(display_frame.frame_id.as_str()) {
            point(&mut v["anchor_surface"]["voxel_center"])?;
            v["anchor_surface"]["frame"]=json!(transform.to_frame_id);
        }
    }
    if let Some(anchors)=view["landmarks"]["anchors"].as_object_mut(){for v in anchors.values_mut(){anchor(&mut v["pose"])?;}}
    if let Some(points)=view["landmarks"]["landmarks"].as_object_mut(){for v in points.values_mut(){
        if v["parent_anchor"].is_null(){point(&mut v["position"])?;v["frame"]=json!(transform.to_frame_id);}
    }}
    view["landmarks"]["frame"]=json!(transform.to_frame_id);
    let robot = |v:&mut Value| -> Result<()> {
        if v.is_null(){return Ok(())}
        for key in ["pose","glyph_pose"] {if !v[key].is_null(){
            let p:auki_odometry::Pose=serde_json::from_value(v[key].clone())?;
            v[key]=serde_json::to_value(compose(&display_to_scene,&p)?)?;
        }}
        v["frame"]=json!(transform.to_frame_id);Ok(())
    };
    robot(&mut view["local_mapping"]["robot_pose"])?;
    if let Some(runners)=view["local_mapping"]["runners"].as_object_mut(){for r in runners.values_mut(){robot(&mut r["robot_pose"])?;}}
    view["frame"]=json!(transform.to_frame_id);
    view["voxel_rotation_xyzw"]=json!(display_to_scene.rotation_xyzw);
    view["display_alignment"]=json!({"product":crate::frame_transform::PRODUCT,"portal":value.evidence.as_ref().map(|e|&e.portal),"native_frame":scene.frame});
    Ok(())
}

fn render_robot_pose(
    robot: &mut Value,
    scene: &CanonicalScene,
    target: &FrameRegistryEntry,
) -> Result<()> {
    let pose = crate::conventions::pose(
        &serde_json::from_value(robot["pose"].clone())?,
        &scene.voxels.definition.frame,
        target,
    )?;
    robot["pose"] = serde_json::to_value(&pose)?;
    robot["frame"] = json!(target.frame_id);

    // Older runners publish the body declaration in the registry instead of
    // alongside each pose. Resolve it by producer, including shared-map inputs;
    // the map's axes do not tell us which body axis means forward.
    let source = if let Some(frame) = robot.get("from_frame") {
        Some(serde_json::from_value::<FrameRegistryEntry>(frame.clone())?)
    } else if let Some(peer) = robot["clock"]["peer_id"].as_str() {
        let registries = &scene.registries;
        let mut frames = registries["frames"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(
                registries["input_registries"][peer]["frames"]
                    .as_array()
                    .into_iter()
                    .flatten(),
            )
            .filter(|f| f["peer_id"] == peer && f["frame_id"] == "robot.base");
        let source = frames.next().cloned();
        ensure!(
            frames.all(|f| Some(f) == source.as_ref()),
            "conflicting robot body declarations"
        );
        source
            .map(serde_json::from_value::<FrameRegistryEntry>)
            .transpose()?
    } else {
        None
    };
    if let Some(source) = source {
        // The browser's illustrative mesh uses ROS body axes. Keep the actual
        // body pose for geometry consumers and add a separate presentation pose.
        let glyph = FrameRegistryEntry::ros_body("home", "viewer.robot-glyph");
        robot["glyph_pose"] =
            serde_json::to_value(crate::conventions::pose_source(&pose, &source, &glyph)?)?;
        robot["glyph_frame"] = serde_json::to_value(glyph)?;
        robot["from_frame"] = serde_json::to_value(source)?;
    }
    Ok(())
}

async fn monitor_scene(
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
                let mut deadline=tokio::time::Instant::now()+Duration::from_secs(10);
                loop {
                    let event=tokio::select! {_=stop.cancelled()=>return Ok(()),event=before_deadline(deadline,async {Ok(subscription.next().await?)})=>event?};
                    match event {
                        Some(RemoteObservationEvent::Observation(o))=>{
                            let s=&o.payload;s.validate(&config.peer_id)?;
                            ensure!(s.frame==config.frame || (crate::local_mapper::is_local_family(&config.frame) && s.frame.starts_with(&format!("{}:",config.frame)) && crate::local_mapper::is_local_frame(&s.frame)),"runner frame differs from connection profile");
                            ensure!(crate::model::clock_ref(&s.publication_clock)==subscription.product().producer.clock,"scene clock mismatch");
                            if !crate::local_mapper::is_local_frame(&s.frame) {ensure!(crate::conventions::matching_portals(&s.portals,portals)?,"Runner scene portal reference differs from Home");}
                            deadline=tokio::time::Instant::now()+Duration::from_secs(10);
                            state.send_modify(|state| {
                                state.scene=Some((**s).clone());state.status="connected".into();state.received=Some(Instant::now());
                                state.detail=if crate::local_mapper::is_local_frame(&s.frame){"Robot-native map · scene alignment is a separate Catalog Product".into()}else{"Remote scene · SDK coordinate conversion".into()};
                            });
                        },
                        Some(RemoteObservationEvent::Gap(_))=>{}, // Complete snapshots replace prior state.
                        _=>bail!("Runner scene stream ended"),
                    }
                }
            }.await;
            let cleanup=tokio::time::timeout(Duration::from_secs(2),subscription.close()).await;
            result.and(match cleanup {
                Ok(value)=>value.map_err(Into::into),
                Err(_)=>Err(anyhow!("Runner scene unsubscribe timed out")),
            })
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

pub async fn monitor(
    peer:&AukiPeer, config:Option<&HomeConnection>, portals:&MapSnapshot,
    state:&watch::Sender<RemoteScene>, stop:&CancellationToken,
) {
    let transform=async {
        if let Some(config)=config {crate::frame_transform::follow(peer,config,state,stop).await;}
        else {stop.cancelled().await;}
    };
    tokio::join!(monitor_scene(peer,config,portals,state,stop),transform);
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[tokio::test]
    async fn silent_stream_expires_instead_of_waiting_forever() {
        let result=before_deadline(tokio::time::Instant::now()+Duration::from_millis(10),
            std::future::pending::<Result<()>>()).await;
        assert!(result.unwrap_err().to_string().contains("rediscovering"));
    }
    #[tokio::test]
    async fn live_snapshot_passes_through() {
        assert_eq!(before_deadline(tokio::time::Instant::now()+Duration::from_secs(1),
            async {Ok(42)}).await.unwrap(),42);
    }
}
