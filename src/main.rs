use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use spatial_home::{
    model::{Contribution, Detection, Event, Store, fixtures},
    raycast::raycast,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

struct RunnerView {
    name: String,
    primary: bool,
    state: tokio::sync::watch::Receiver<spatial_home::scene_mirror::RemoteScene>,
}
struct AppState {
    local_scenes: Arc<spatial_home::local_scenes::LocalScenes>,
    simulation_disconnect: tokio_util::sync::CancellationToken,
    simulation:
        Option<tokio::sync::watch::Receiver<spatial_home::simulated_robot::SimulationState>>,
    fiducials: Mutex<spatial_home::fiducials::FiducialStore>,
    picks: Mutex<spatial_home::picking::Picks>,
    occupancy: spatial_home::robot_occupancy::Cache,
    ar_frames: spatial_home::ar_preview::Frames,
    previews: tokio::sync::mpsc::Sender<spatial_home::sensor_preview::Request>,
    controls: tokio::sync::mpsc::Sender<spatial_home::component_control::Request>,
    pointing: tokio::sync::mpsc::Sender<spatial_home::robot_pointing::Invoke>,
    store: Arc<Mutex<Store>>,
    changes: tokio::sync::watch::Sender<()>,
    peer: tokio::sync::watch::Receiver<Value>,
    runner: tokio::sync::watch::Receiver<spatial_home::runner_pose::RunnerState>,
    mapping: tokio::sync::watch::Receiver<Value>,
    remote_scene: tokio::sync::watch::Receiver<spatial_home::scene_mirror::RemoteScene>,
    home_live: tokio::sync::watch::Receiver<spatial_home::scene_mirror::RemoteScene>,
    runners: std::collections::BTreeMap<String, RunnerView>,
}
type Shared = Arc<AppState>;
struct ApiError(anyhow::Error);
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error":self.0.to_string()})),
        )
            .into_response()
    }
}
type ApiResult = Result<Json<Value>, ApiError>;
fn lock(state: &Shared) -> Result<std::sync::MutexGuard<'_, Store>, ApiError> {
    state
        .store
        .lock()
        .map_err(|_| ApiError(anyhow::anyhow!("state unavailable")))
}

#[derive(Deserialize)]
struct PreviewQuery {
    home_frame: Option<String>,
    #[serde(default)]
    debug_pose: bool,
    #[serde(default)]
    follow: bool,
    expected_manifest: Option<String>,
    sensor: Option<String>,
    #[serde(default)]
    scene: bool,
    #[serde(default)]
    ar: bool,
    #[serde(default)]
    pick: bool,
}
async fn component_enabled(State(s):State<Arc<AppState>>,headers:HeaderMap,
    Json(command):Json<spatial_home::component_control::Command>)->ApiResult {
    local_write(&headers)?;
    let (reply,receive)=tokio::sync::oneshot::channel();
    s.controls.try_send(spatial_home::component_control::Request {command,reply})
        .map_err(|_|anyhow::anyhow!("Device controls busy or unavailable"))?;
    let result=tokio::time::timeout(std::time::Duration::from_secs(8),receive).await
        .map_err(|_|anyhow::anyhow!("Control timed out; state unconfirmed"))?
        .map_err(|_|anyhow::anyhow!("Device controls stopped"))??;
    Ok(Json(json!({"enabled":result.enabled})))
}
async fn sensor_preview(
    State(s): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<PreviewQuery>,
) -> Result<Response,ApiError> {
    let (reply, receive) = tokio::sync::oneshot::channel();
    s.previews
        .try_send(spatial_home::sensor_preview::Request {
            home_frame: q.home_frame,
            debug_pose: q.debug_pose,
            sensor: q.sensor,
            scene: q.scene,
            ar: q.ar,
            pick: q.pick,
            follow: q.follow,
            capture: false,
            expected_manifest: q.expected_manifest,
            reply,
        })
        .map_err(|_| anyhow::anyhow!("Sensor connection unavailable"))?;
    let response = tokio::time::timeout(std::time::Duration::from_secs(10), receive)
        .await
        .map_err(|_| anyhow::anyhow!("Sensor request timed out"))?
        .map_err(|_| anyhow::anyhow!("Sensor connection stopped"))??;
    let mut result=response.value;
    if let Some(geometry) = result.as_object_mut().and_then(|o| o.remove("picking")) {
        let token = s
            .picks
            .lock()
            .map_err(|_| anyhow::anyhow!("Picking unavailable"))?
            .remember_camera(geometry)?;
        result["camera_token"] = json!(token);
    }
    if let Some(sensors) = result.get_mut("sensors").and_then(Value::as_array_mut) {
        for sensor in sensors {
            if let Some(runner) = sensor["peer_id"].as_str().and_then(|id| s.runners.get(id)) {
                sensor["label"] = json!(format!(
                    "{} · {}",
                    runner.name,
                    sensor["label"].as_str().unwrap_or("Camera")
                ));
            }
        }
    }
    if let Some(bytes)=response.image {
        let packet=spatial_home::camera_frame::browser_packet(&result,&bytes)?;
        return Ok(([(header::CONTENT_TYPE,spatial_home::camera_frame::BROWSER_MIME),
            (header::CACHE_CONTROL,"no-store")],packet).into_response());
    }
    Ok(Json(result).into_response())
}
#[derive(Deserialize)]
struct RunnerQuery {
    peer: Option<String>,
}
async fn robot_occupancy(
    State(s): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<RunnerQuery>,
) -> ApiResult {
    let remote = if q.peer.is_some() {
        selected_runner(&s, &q)?
    } else {
        &s.home_live
    };
    let remote = remote.borrow();
    let Some(scene) = remote.scene.as_ref() else {
        return Ok(Json(json!({"robots":[],"status":"waiting for scene"})));
    };
    if remote.status_view()["status"] != "connected" {
        return Ok(Json(json!({"robots":[],"status":"scene stale"})));
    };
    let samples = s
        .occupancy
        .lock()
        .map_err(|_| anyhow::anyhow!("Occupancy cache unavailable"))?;
    let mut robots = Vec::new();
    let mut unavailable = Vec::new();
    for (peer, history) in samples
        .iter()
        .filter(|(peer, _)| q.peer.as_ref().is_none_or(|p| p == *peer))
    {
        let Some(sample) = history.back() else {
            continue;
        };
        match spatial_home::robot_occupancy::render_latest(history, &s.ar_frames, scene, q.peer.is_none()) {
            Ok(mut view) => {
                if let Some(transform) = &remote.frame_transform
                    && transform.usable(scene)
                {
                    let (frame, alignment) = spatial_home::scene_mirror::display_transform(
                        scene, &transform.value, view["frame"].as_str().ok_or_else(||anyhow::anyhow!("Missing body display frame"))?)?;
                    let pose = serde_json::from_value(view["pose"].clone()).map_err(anyhow::Error::from)?;
                    view["pose"] = serde_json::to_value(spatial_home::local_mapper::compose(&alignment, &pose)?).map_err(anyhow::Error::from)?;
                    view["frame"] = json!(frame);
                }
                view["name"] = json!(
                    s.runners
                        .get(peer)
                        .map(|r| r.name.as_str())
                        .unwrap_or("Galbot")
                );
                robots.push(view);
            }
            Err(e) => unavailable.push(json!({"peer_id":peer,"detail":e.to_string(),"sequence":sample.sequence,"capture_timestamp_ns":sample.timestamp_ns,"received_age_ms":sample.received.elapsed().as_millis(),"compute_ms":sample.body.compute_ms})),
        }
    }
    Ok(Json(json!({"robots":robots,"unavailable":unavailable})))
}
fn selected_runner<'a>(
    s: &'a AppState,
    q: &RunnerQuery,
) -> anyhow::Result<&'a tokio::sync::watch::Receiver<spatial_home::scene_mirror::RemoteScene>> {
    match q.peer.as_deref() {
        None => Ok(&s.remote_scene),
        Some(id) => s
            .runners
            .get(id)
            .map(|r| &r.state)
            .ok_or_else(|| anyhow::anyhow!("Unknown runner peer")),
    }
}
async fn remote_scene(
    State(s): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<RunnerQuery>,
) -> ApiResult {
    Ok(Json(selected_runner(&s, &q)?.borrow().render()?))
}
async fn remote_snapshot(
    State(s): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<RunnerQuery>,
) -> ApiResult {
    let r = selected_runner(&s, &q)?.borrow();
    let scene = r
        .scene
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Runner scene not available"))?;
    Ok(Json(
        serde_json::to_value(scene).map_err(anyhow::Error::from)?,
    ))
}
async fn home_live_scene(State(s): State<Shared>) -> ApiResult {
    let live = s.home_live.borrow();
    if live.scene.is_none() {
        let store = lock(&s)?;
        let mut view = serde_json::to_value(&store.built.view).map_err(anyhow::Error::from)?;
        view["landmarks"] = store.built.registries["landmarks"].clone();
        view["notes"] = json!(
            spatial_home::notes::from_registries(
                &store.built.registries,
                spatial_home::model::FRAME
            )?
            .items
        );
        spatial_home::mobile_pose::append_labels(
            &mut view,
            &store.built.registries["mobile_presence"],
        );
        view["mode"] = json!("Home · waiting for runner geometry");
        view["remote_status"] = live.status_view();
        return Ok(Json(view));
    }
    let mut view = live.render()?;
    {
        let store = lock(&s)?;
        spatial_home::mobile_pose::append_labels(
            &mut view,
            &store.built.registries["mobile_presence"],
        );
    }
    view["authority"] = json!("Home mapper");
    view["mode"] = json!("Home-built scenegraph from raw runner observations");
    Ok(Json(view))
}
async fn home_live_snapshot(State(s): State<Shared>) -> ApiResult {
    Ok(Json(
        serde_json::to_value(
            s.home_live
                .borrow()
                .scene
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Home live map waiting for portal observations"))?,
        )
        .map_err(anyhow::Error::from)?,
    ))
}
async fn home_live_status(State(s): State<Shared>) -> ApiResult {
    Ok(Json(s.home_live.borrow().status_view()))
}
async fn scene_sources(State(s): State<Shared>) -> ApiResult {
    let mut status = s.remote_scene.borrow().status_view();
    let mut local = vec![json!({"id":"demo","name":spatial_home::environments::reference_name()})];
    local.extend(s.local_scenes.rows());
    if !lock(&s)?.demo && let Some(preferred) = s.local_scenes.preferred_id() {
        status["preferred"] = json!(preferred);
    }
    if let Some(simulation) = &s.simulation {
        let simulation = simulation.borrow();
        if simulation.status == "running" && simulation.voxels.is_some() {
            local.push(json!({"id":"sim-voxels","name":"Simulated robot · voxels","available":true,"status":"running"}));
        }
        if simulation.occupancy.is_some() {
            local.push(json!({"id":"sim-occupancy","name":"Home · accumulated occupancy 2D","available":true,"status":if simulation.status == "running" {"live"} else {"retained"}}));
        }
    }
    if s.home_live.borrow().scene.is_some() {
        local.push(json!({"id":"home-live","name":"Home live map"}));
    }
    status["local"] = json!(local);
    status["phones"] = spatial_home::mobile_local::sources();
    // Catalog listing opens no scene subscriptions or sensor streams.
    status["scenes"] = json!([]);
    let (reply, receive) = tokio::sync::oneshot::channel();
    if s.previews.try_send(spatial_home::sensor_preview::Request {home_frame:None,debug_pose:false,sensor:None, scene:true, ar:false, pick:false, follow:false, capture:false, expected_manifest:None, reply}).is_ok() {
        if let Ok(Ok(Ok(value))) = tokio::time::timeout(std::time::Duration::from_secs(2), receive).await {
            status["scenes"] = value["scenes"].clone();
        }
    }
    status["runners"] = json!(
        s.runners
            .iter()
            .map(|(id, r)| {
                let mut view = r.state.borrow().status_view();
                view["peer_id"] = json!(id);
                view["name"] = json!(r.name);
                view["primary"] = json!(r.primary);
                view
            })
            .collect::<Vec<_>>()
    );
    let aliases=spatial_home::peer_aliases::all()?;
    for key in ["runners","phones"]{if let Some(entries)=status[key].as_array_mut(){for entry in entries{if let Some(alias)=entry["peer_id"].as_str().and_then(|id|aliases.get(id)){entry["name"]=json!(alias);}}}}
    if let Some(entries)=status["scenes"].as_array_mut(){for entry in entries{entry["peer_alias"]=entry["peer_id"].as_str().and_then(|id|aliases.get(id)).map(|s|json!(s)).unwrap_or(Value::Null);}}
    Ok(Json(status))
}

async fn phone_local_scene(axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String,String>>) -> ApiResult {
    Ok(Json(spatial_home::mobile_local::render(q.get("peer").ok_or_else(|| anyhow::anyhow!("Missing phone peer"))?)?))
}
#[derive(Deserialize)]
struct SimulationQuery {
    snapshot: Option<bool>,
}
async fn disconnect_simulated_robots(State(s): State<Shared>, headers: HeaderMap) -> ApiResult {
    local_write(&headers)?;
    if s.simulation.is_none() {
        return Err(anyhow::anyhow!("Simulation is not enabled").into());
    }
    s.simulation_disconnect.cancel();
    Ok(Json(json!({"status":"disconnecting","home":"running"})))
}
async fn simulation_scene(
    State(s): State<Shared>,
    axum::extract::Path(layer): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<SimulationQuery>,
) -> ApiResult {
    let state = s
        .simulation
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Robot simulator is not enabled"))?
        .borrow();
    let snapshot = q.snapshot.unwrap_or(false);
    if layer == "voxels" && state.status != "running" {
        return Err(anyhow::anyhow!("Robot voxel publishers unavailable: {}", state.status).into());
    }
    let value = match (layer.as_str(), snapshot) {
        ("voxels", true) => state.source_snapshot.clone(),
        ("occupancy", true) => state
            .occupancy_snapshot
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(anyhow::Error::from)?,
        ("voxels", false) => state.voxels.clone(),
        ("occupancy", false) => state.occupancy.clone(),
        _ => return Err(anyhow::anyhow!("Unknown simulation layer").into()),
    };
    let mut value =
        value.ok_or_else(|| anyhow::anyhow!("Simulator {}: {}", state.status, state.detail))?;
    if !snapshot {
        value["simulation_status"] = json!(state.status);
        value["simulation_detail"] = json!(state.detail);
        if state.status != "running" {
            value["remote_status"] = json!({"status":"stale","age_ms":null});
        }
    }
    Ok(Json(value))
}
async fn scene(State(s): State<Shared>) -> ApiResult {
    let store = lock(&s)?;
    let mut view = serde_json::to_value(&store.built.view).map_err(anyhow::Error::from)?;
    view["environment_name"] = json!(spatial_home::environments::reference_name());
    view["volumes"] = json!(spatial_home::notes::from_registries(&store.built.registries, spatial_home::model::FRAME)?.volumes);
    view["landmarks"] = store.built.registries["landmarks"].clone();
    view["semantic_volumes"] = serde_json::to_value(s.local_scenes.volumes.get("canonical", spatial_home::model::FRAME)?).map_err(anyhow::Error::from)?;
    view["volumes"] = json!(s.local_scenes.volumes.mobile("canonical",spatial_home::model::FRAME)?);
    view["notes"] = json!(
        spatial_home::notes::from_registries(&store.built.registries, spatial_home::model::FRAME)?
            .items
    );
    spatial_home::mobile_pose::append_labels(&mut view, &store.built.registries["mobile_presence"]);
    Ok(Json(view))
}
#[derive(Deserialize)]
struct VolumeQuery { scene: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathWrite { frame: String, stroke: spatial_home::scene_paths::Stroke }
async fn get_paths(State(s):State<Shared>,axum::extract::Query(q):axum::extract::Query<VolumeQuery>)->ApiResult {
    let (key,frame)=volume_context(&s,&q.scene)?;
    Ok(Json(json!(s.local_scenes.paths.get(&key,&frame)?)))
}
async fn create_path(State(s):State<Shared>,headers:HeaderMap,axum::extract::Query(q):axum::extract::Query<VolumeQuery>,Json(value):Json<PathWrite>)->ApiResult {
    local_write(&headers)?;
    if !q.scene.starts_with("local:") { return Err(anyhow::anyhow!("Choose a local Home scene to draw").into()); }
    let (key,frame)=volume_context(&s,&q.scene)?;
    if frame!=value.frame { return Err(anyhow::anyhow!("Scene frame changed; redraw the path").into()); }
    let id=value.stroke.id.clone();
    Ok(Json(json!(s.local_scenes.paths.change(&key,&frame,&id,Some(value.stroke))?)))
}
async fn delete_path(State(s):State<Shared>,headers:HeaderMap,axum::extract::Path(id):axum::extract::Path<String>,axum::extract::Query(q):axum::extract::Query<VolumeQuery>)->ApiResult {
    local_write(&headers)?;
    if !q.scene.starts_with("local:") { return Err(anyhow::anyhow!("Choose a local Home scene").into()); }
    let (key,frame)=volume_context(&s,&q.scene)?;
    Ok(Json(json!(s.local_scenes.paths.change(&key,&frame,&id,None)?)))
}
async fn extend_path(State(s):State<Shared>,headers:HeaderMap,axum::extract::Path(id):axum::extract::Path<String>,axum::extract::Query(q):axum::extract::Query<VolumeQuery>,Json(value):Json<PathWrite>)->ApiResult {
    local_write(&headers)?;
    if !q.scene.starts_with("local:") { return Err(anyhow::anyhow!("Choose a local Home scene").into()); }
    let (key,frame)=volume_context(&s,&q.scene)?;
    if frame!=value.frame || id!=value.stroke.id { return Err(anyhow::anyhow!("Path frame or identity changed").into()); }
    Ok(Json(json!(s.local_scenes.paths.extend(&key,&frame,value.stroke)?)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VolumeWrite {
    frame: String,
    volume: Option<spatial_home::semantic_volumes::Volume>,
}
fn volume_context(s: &Shared, scene: &str) -> Result<(String, String), ApiError> {
    if let Some(id) = scene.strip_prefix("local:") {
        let snapshot = s.local_scenes.snapshot(id)?;
        Ok((id.into(), snapshot.scenegraph.map.frame.id.clone()))
    } else {
        if scene != "demo" { return Err(anyhow::anyhow!("Choose a local Home scene to edit volumes").into()); }
        Ok(("canonical".into(), spatial_home::model::FRAME.into()))
    }
}
async fn get_volumes(State(s): State<Shared>, axum::extract::Query(q): axum::extract::Query<VolumeQuery>) -> ApiResult {
    let (key, frame) = volume_context(&s, &q.scene)?;
    Ok(Json(serde_json::to_value(s.local_scenes.volumes.get(&key, &frame)?).map_err(anyhow::Error::from)?))
}
fn write_volume(s: &Shared, scene: &str, id: Option<&str>, value: VolumeWrite) -> ApiResult {
    let (key, frame) = volume_context(s, scene)?;
    let volumes = s.local_scenes.volumes.change(&key, &frame, id, value.volume, &value.frame)?;
    if key == "canonical" { lock(s)?.built.registries["semantic_volumes"] = json!(volumes); s.changes.send_replace(()); }
    Ok(Json(json!(volumes)))
}
async fn create_volume(State(s): State<Shared>, headers: HeaderMap, axum::extract::Query(q): axum::extract::Query<VolumeQuery>, Json(mut value): Json<VolumeWrite>) -> ApiResult {
    local_write(&headers)?;
    let volume = value.volume.as_mut().ok_or_else(|| anyhow::anyhow!("Missing volume"))?;
    volume.id = uuid::Uuid::new_v4().to_string();
    write_volume(&s, &q.scene, None, value)
}
async fn update_volume(State(s): State<Shared>, headers: HeaderMap, axum::extract::Path(id): axum::extract::Path<String>, axum::extract::Query(q): axum::extract::Query<VolumeQuery>, Json(value): Json<VolumeWrite>) -> ApiResult {
    local_write(&headers)?;
    if value.volume.is_none() { return Err(anyhow::anyhow!("Missing volume").into()); }
    write_volume(&s, &q.scene, Some(&id), value)
}
async fn delete_volume(State(s): State<Shared>, headers: HeaderMap, axum::extract::Path(id): axum::extract::Path<String>, axum::extract::Query(q): axum::extract::Query<VolumeQuery>, Json(value): Json<VolumeWrite>) -> ApiResult {
    local_write(&headers)?;
    if value.volume.is_some() { return Err(anyhow::anyhow!("Delete cannot contain a volume").into()); }
    write_volume(&s, &q.scene, Some(&id), value)
}
async fn local_scene(
    State(s): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult {
    Ok(Json(s.local_scenes.view(&id)?))
}
async fn local_presence(State(s): State<Shared>, axum::extract::Path(id): axum::extract::Path<String>) -> ApiResult {
    Ok(Json(s.local_scenes.presence(&id)?))
}
async fn local_snapshot(
    State(s): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult {
    let editor=s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene unavailable"))?.get(&id).cloned();
    let snapshot=if let Some(editor)=editor{editor.reference()?}else{s.local_scenes.snapshot(&id)?.clone()};
    Ok(Json(serde_json::to_value(snapshot).map_err(anyhow::Error::from)?))
}
// Reset is an explicit, snapshot-checked write, never part of normal startup.
async fn reset_local_mapping(State(s):State<Shared>,headers:HeaderMap,axum::extract::Path(id):axum::extract::Path<String>,Json(expected):Json<auki_scenegraph::MapSnapshot>)->ApiResult{
    local_write(&headers)?;
    expected.validate().map_err(anyhow::Error::from)?;
    let editor=s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene unavailable"))?.get(&id).cloned().ok_or_else(||anyhow::anyhow!("Unknown local scene"))?;
    Ok(Json(editor.reset_mapping(&expected)?))
}
async fn delete_note(
    State(s): State<Shared>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String,String>>,
) -> ApiResult {
    local_write(&headers)?;
    if let Some(scene)=query.get("scene") {
        let editor=s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene annotations unavailable"))?
            .get(scene).cloned().ok_or_else(||anyhow::anyhow!("Scene annotations unavailable"))?;
        editor.change(&id,None)?;
        return Ok(Json(json!({"deleted":id})));
    }
    let mut store = lock(&s)?;
    spatial_home::notes::remove(&mut store, &id)?;
    s.changes.send_replace(());
    Ok(Json(json!({"deleted":id})))
}
async fn rename_note(
    State(s): State<Shared>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String,String>>,
    Json(request): Json<serde_json::Value>,
) -> ApiResult {
    local_write(&headers)?;
    let text = request["text"].as_str().unwrap_or_default().to_owned();
    let details = request
        .get("details")
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(anyhow::Error::from)?;
    if let Some(scene)=query.get("scene") {
        let editor=s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene annotations unavailable"))?
            .get(scene).cloned().ok_or_else(||anyhow::anyhow!("Scene annotations unavailable"))?;
        return Ok(Json(json!(editor.change(&id,Some((text,details)))?)));
    }
    let mut store = lock(&s)?;
    let note = spatial_home::notes::update(&mut store, &id, text, details)?;
    s.changes.send_replace(());
    Ok(Json(json!(note)))
}
#[derive(Deserialize)]
struct NewNoteRequest {
    text: String,
    portal: String,
    height_m: f64,
    #[serde(default)]
    details: spatial_home::notes::NoteDetails,
}
async fn add_note(
    State(s): State<Shared>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String,String>>,
    Json(note): Json<NewNoteRequest>,
) -> ApiResult {
    local_write(&headers)?;
    if let Some(scene)=query.get("scene") {
        let view=s.local_scenes.view(scene)?;
        let portal=view["portals"].as_array().and_then(|p|p.iter().find(|p|p["anchor_id"]==note.portal)).ok_or_else(||anyhow::anyhow!("Unknown scene portal"))?;
        let mut position:[f64;3]=serde_json::from_value(portal["pose_in_map"]["translation"].clone()).map_err(anyhow::Error::from)?;
        position[1]+=note.height_m;
        let editor=s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene annotations unavailable"))?.get(scene).cloned().ok_or_else(||anyhow::anyhow!("Scene annotations unavailable"))?;
        return Ok(Json(json!(editor.create(spatial_home::notes::TextNote{id:uuid::Uuid::new_v4().to_string(),text:note.text.trim().into(),details:note.details,position,anchor_portal:note.portal,anchor_robot:None,anchor_surface:None,height_m:note.height_m})?)));
    }
    let mut store = lock(&s)?;
    let note = spatial_home::notes::add_detailed(
        &mut store,
        spatial_home::notes::NewNote {
            text: note.text,
            portal: note.portal,
            height_m: note.height_m,
        },
        note.details,
    )?;
    s.changes.send_replace(());
    Ok(Json(json!(note)))
}
async fn domain_scene(State(s): State<Shared>) -> ApiResult {
    let s = lock(&s)?;
    let map = &s.built.domain_portals;
    Ok(Json(
        json!({"frame":map.scenegraph.map.frame.id,"domain":map.scenegraph.map.domain_reference,"convention":"OpenGL · Y up · meters","voxel_size":0.1,"voxels":[],"objects":[],"sources":{},"integrated_observations":0,"portals":map.scenegraph.anchors.values().collect::<Vec<_>>(),"mode":"Stored domain poses · heights unverified"}),
    ))
}
async fn domain_snapshot(State(s): State<Shared>) -> ApiResult {
    Ok(Json(
        serde_json::to_value(&lock(&s)?.built.domain_portals).map_err(anyhow::Error::from)?,
    ))
}
async fn fixture() -> ApiResult {
    Ok(Json(fixtures()?))
}
async fn portal_sizes() -> ApiResult {
    let sizes = spatial_home::portal_sizes::PortalSizes::load("config/portals.json")?;
    Ok(Json(
        serde_json::to_value(sizes).map_err(anyhow::Error::from)?,
    ))
}
async fn fiducial_catalog(State(s): State<Shared>) -> ApiResult {
    let store = s
        .fiducials
        .lock()
        .map_err(|_| anyhow::anyhow!("fiducial registry unavailable"))?;
    Ok(Json(spatial_home::fiducials::catalog_view(&store)))
}
#[derive(Deserialize)]
struct LandmarkQuery {
    landmark_type: Option<String>,
}
async fn landmark_catalog(
    State(s): State<Shared>,
    axum::extract::Query(query): axum::extract::Query<LandmarkQuery>,
) -> ApiResult {
    let store = lock(&s)?;
    let mut landmarks = spatial_home::landmarks::from_registries(
        &store.built.registries,
        &store.built.portals.scenegraph.map.frame.id,
    )?;
    if let Some(kind) = query.landmark_type {
        landmarks
            .landmarks
            .retain(|_, landmark| landmark.recognition.landmark_type == kind);
        // Keep all parents available to resolve the filtered positions.
    }
    Ok(Json(
        serde_json::to_value(landmarks).map_err(anyhow::Error::from)?,
    ))
}
async fn create_fiducial(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<spatial_home::fiducials::CreateFiducial>,
) -> ApiResult {
    local_write(&headers)?;
    let mut fiducials = s
        .fiducials
        .lock()
        .map_err(|_| anyhow::anyhow!("fiducial registry unavailable"))?;
    let mut store = lock(&s)?;
    let marker = fiducials.create(request)?;
    store.built.registries["fiducials"] = spatial_home::fiducials::registry_view(&fiducials);
    s.changes.send_replace(());
    Ok(Json(spatial_home::fiducials::marker_view(&marker)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FiducialMeasurement {
    side_length_m: f64,
}
async fn resize_fiducial(
    State(s): State<Shared>,
    headers: HeaderMap,
    axum::extract::Path((family, id)): axum::extract::Path<(String, u32)>,
    Json(request): Json<FiducialMeasurement>,
) -> ApiResult {
    local_write(&headers)?;
    let family = spatial_home::fiducials::FiducialFamily::parse_slug(&family)
        .ok_or_else(|| anyhow::anyhow!("unknown fiducial family"))?;
    let mut fiducials = s
        .fiducials
        .lock()
        .map_err(|_| anyhow::anyhow!("fiducial registry unavailable"))?;
    let mut store = lock(&s)?;
    let marker = fiducials.resize(family, id, request.side_length_m)?;
    store.built.registries["fiducials"] = spatial_home::fiducials::registry_view(&fiducials);
    s.changes.send_replace(());
    Ok(Json(spatial_home::fiducials::marker_view(&marker)))
}
fn requested_fiducial(
    state: &Shared,
    family: &str,
    id: u32,
) -> Result<spatial_home::fiducials::Fiducial, ApiError> {
    let family = spatial_home::fiducials::FiducialFamily::parse_slug(family)
        .ok_or_else(|| anyhow::anyhow!("unknown fiducial family"))?;
    state
        .fiducials
        .lock()
        .map_err(|_| anyhow::anyhow!("fiducial registry unavailable"))?
        .find(family, id)
        .cloned()
        .ok_or_else(|| ApiError(anyhow::anyhow!("fiducial is not registered")))
}
async fn fiducial_svg(
    State(s): State<Shared>,
    axum::extract::Path((family, id)): axum::extract::Path<(String, u32)>,
) -> Result<Response, ApiError> {
    let marker = requested_fiducial(&s, &family, id)?;
    let svg = spatial_home::fiducials::printable_svg(&marker)?;
    let filename = format!(
        "{}-{}-{}mm.svg",
        marker.family.slug(),
        marker.id,
        (marker.side_length_m * 1000.).round()
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("image/svg+xml; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_str(&format!("inline; filename=\"{filename}\""))
            .map_err(anyhow::Error::from)?,
    );
    Ok((headers, svg).into_response())
}
async fn fiducial_print(
    State(s): State<Shared>,
    axum::extract::Path((family, id)): axum::extract::Path<(String, u32)>,
) -> Result<Html<String>, ApiError> {
    let marker = requested_fiducial(&s, &family, id)?;
    let svg = spatial_home::fiducials::printable_svg(&marker)?;
    let side_mm = marker.side_length_m * 1000.;
    Ok(Html(format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>{} ID {}</title><style>
        body{{margin:12mm;font:14px system-ui;color:#18181e}}.controls{{margin-bottom:10mm}}button{{padding:8px 14px}}svg{{display:block}}
        @media print{{.controls{{display:none}}body{{margin:0}}}}@page{{margin:12mm}}
        </style></head><body><div class="controls"><strong>{} ID {}</strong><p>Outer black square: {side_mm:.1} mm. Print at 100% / Actual size; disable “Fit to page”. The white quiet zone is additional.</p><button onclick="window.print()">Print marker</button></div>{svg}</body></html>"#,
        marker.family.label(),
        marker.id,
        marker.family.label(),
        marker.id,
    )))
}
async fn catalog(State(s): State<Shared>) -> ApiResult {
    Ok(Json(lock(&s)?.built.catalog.clone()))
}
async fn mapping_status(State(s): State<Shared>) -> ApiResult {
    Ok(Json(s.mapping.borrow().clone()))
}
async fn runner_pose(State(s): State<Shared>) -> ApiResult {
    Ok(Json(s.runner.borrow().view()))
}
async fn peer_status(State(s): State<Shared>) -> ApiResult {
    if let Some(simulation) = &s.simulation {
        let state = simulation.borrow();
        return Ok(Json(if state.network.is_null() {
            json!({"enabled":true,"simulation":true,"status":format!("Simulation · {}",state.status),"detail":state.detail})
        } else {
            state.network.clone()
        }));
    }
    Ok(Json(spatial_home::peer_aliases::network(s.peer.borrow().clone())?))
}
async fn set_peer_alias(State(s):State<Shared>,headers:HeaderMap,Json(args):Json<serde_json::Value>)->ApiResult{
 local_write(&headers)?;Ok(Json(peer_alias_edit(&s,args)?))
}
fn peer_alias_edit(s:&Shared,args:serde_json::Value)->anyhow::Result<Value>{
 #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Alias {peer_id:String,alias:String}
 let a:Alias=serde_json::from_value(args)?;let network=s.peer.borrow();
 anyhow::ensure!(network["connection"]["peer_id"]==a.peer_id||network["peers"].as_array().is_some_and(|peers|peers.iter().any(|p|p["peer_id"]==a.peer_id))||spatial_home::peer_aliases::all()?.contains_key(&a.peer_id),"Peer is not in Home's cluster");
 spatial_home::peer_aliases::set(&a.peer_id,&a.alias)
}
async fn home_connection(State(s): State<Shared>) -> ApiResult {
    let state = s.peer.borrow();
    let profile = state
        .get("connection")
        .filter(|_| state["status"] == "Ready")
        .ok_or_else(|| ApiError(anyhow::anyhow!("Home P2P peer is not ready")))?;
    Ok(Json(profile.clone()))
}
async fn local_scene_connection(State(s): State<Shared>, axum::extract::Path(id): axum::extract::Path<String>) -> ApiResult {
    let state = s.peer.borrow();
    let value = state.get("connection").filter(|_| state["status"] == "Ready")
        .ok_or_else(|| anyhow::anyhow!("Home P2P peer is not ready"))?;
    let home = serde_json::from_value(value.clone()).map_err(anyhow::Error::from)?;
    Ok(Json(serde_json::to_value(s.local_scenes.connection(&id, home)?).map_err(anyhow::Error::from)?))
}
async fn snapshots(State(s): State<Shared>) -> ApiResult {
    let s = lock(&s)?;
    Ok(Json(
        json!({"domain_portals":s.built.domain_portals,"portals":s.built.portals,"voxels":s.built.voxels,"objects":s.built.view.objects,"registries":s.built.registries}),
    ))
}
// LAN UI control boundary: explicit hosts and same-origin writes; no CORS.
#[cfg(test)]
mod lan_ui_tests {
    use super::*;

    #[test]
    fn default_listener_accepts_lan() {
        assert_eq!(ui_bind_address(None).unwrap().to_string(), "0.0.0.0");
        assert_eq!(ui_bind_address(Some("127.0.0.1")).unwrap().to_string(), "127.0.0.1");
        assert!(ui_bind_address(Some("invalid")).is_err());
    }

    #[test]
    fn lan_writes_require_allowed_host_and_matching_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "192.168.1.26:4329".parse().unwrap());
        headers.insert(header::ORIGIN, "http://192.168.1.26:4329".parse().unwrap());
        assert!(ui_write(&headers, "192.168.1.26:4329").is_ok());
        assert!(ui_write(&headers, "").is_err());
        headers.insert(header::ORIGIN, "http://foreign.example".parse().unwrap());
        assert!(ui_write(&headers, "192.168.1.26:4329").is_err());
        headers.remove(header::ORIGIN);
        headers.insert(header::HOST, "localhost:4329".parse().unwrap());
        assert!(ui_write(&headers, "").is_ok());
    }
}

fn ui_bind_address(configured: Option<&str>) -> Result<std::net::IpAddr, std::net::AddrParseError> {
    configured.unwrap_or("0.0.0.0").parse()
}

fn local_write(headers: &HeaderMap) -> Result<(), ApiError> {
    ui_write(headers, &std::env::var("SPATIAL_UI_HOSTS").unwrap_or_default())
}
// LAN exposure does not remove the Host and same-origin write protections.
fn ui_write(headers: &HeaderMap, lan_hosts: &str) -> Result<(), ApiError> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let authority = host.parse::<axum::http::uri::Authority>().ok();
    let allowed = authority.as_ref().is_some_and(|a| {
        a.port_u16().is_some()
            && (matches!(a.host(), "127.0.0.1" | "localhost" | "[::1]")
                || lan_hosts.split(',').any(|allowed| allowed.trim() == host))
    });
    if !allowed {
        return Err(ApiError(anyhow::anyhow!("UI Host is not allowed")));
    }
    if let Some(origin) = headers.get(header::ORIGIN)
        && origin.to_str().ok() != Some(format!("http://{host}").as_str())
    {
        return Err(ApiError(anyhow::anyhow!("same-origin requests only")));
    }
    Ok(())
}
async fn contribute(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(c): Json<Contribution>,
) -> ApiResult {
    local_write(&headers)?;
    let mut store = lock(&s)?;
    let applied = store.apply(Event::Contribution(c))?;
    if applied {
        s.changes.send_replace(());
    }
    Ok(Json(json!({"applied":applied})))
}
async fn detect(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(d): Json<Detection>,
) -> ApiResult {
    local_write(&headers)?;
    let mut store = lock(&s)?;
    let applied = store.apply(Event::Detection(d))?;
    if applied {
        s.changes.send_replace(());
    }
    Ok(Json(json!({"applied":applied,})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RayRequest {
    frame: String,
    origin: [f64; 3],
    direction: [f64; 3],
    max_distance: f64,
}
async fn cast(State(s): State<Shared>, headers: HeaderMap, Json(r): Json<RayRequest>) -> ApiResult {
    local_write(&headers)?;
    if r.frame != spatial_home::model::FRAME {
        return Err(ApiError(anyhow::anyhow!("ray must be in canonical frame")));
    }
    let s = lock(&s)?;
    Ok(Json(
        json!({"hit":raycast(&s.built.view.voxels,s.built.view.voxel_size,r.origin,r.direction,r.max_distance)?}),
    ))
}
async fn label_pick(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<spatial_home::picking::PickRequest>,
) -> ApiResult {
    local_write(&headers)?;
    let resolved;
    let scene_request = match &request {
        spatial_home::picking::PickRequest::Camera {camera_token,pixel} => {
            resolved = s.picks.lock().map_err(|_|anyhow::anyhow!("Picking unavailable"))?.camera_ray(camera_token,*pixel)?;
            &resolved
        }
        _ => &request,
    };
    if let spatial_home::picking::PickRequest::Scene{frame,origin,direction}=scene_request {
        if let Some(scene)=spatial_home::local_scenes::frame_scene_id(frame) {
            let expected=s.local_scenes.snapshot(scene)?.scenegraph.map.frame.id.clone();
            if *frame!=expected { return Err(ApiError(anyhow::anyhow!("Annotation belongs to another scene"))); }
            let editor=s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene annotations unavailable"))?.get(scene).cloned().ok_or_else(||anyhow::anyhow!("Scene annotations unavailable"))?;
            let mut picks=s.picks.lock().map_err(|_|anyhow::anyhow!("Picking unavailable"))?;
            return Ok(Json(editor.pick(&mut picks,*origin,*direction)?));
        }
    }
    let live = s.home_live.borrow();
    if live.status_view()["status"] != "connected" {
        if let spatial_home::picking::PickRequest::Scene {
            frame,
            origin,
            direction,
        } = scene_request
            && frame == spatial_home::model::FRAME
        {
            let (voxels, size) = {
                let store = lock(&s)?;
                (store.built.view.voxels.clone(), store.built.view.voxel_size)
            };
            return Ok(Json(
                s.picks
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Picking unavailable"))?
                    .pick_canonical(*origin, *direction, &voxels, size)?,
            ));
        }
        return Err(ApiError(anyhow::anyhow!("Home live voxel map unavailable")));
    }
    let scene = live
        .scene
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Waiting for Home live voxels"))?;
    Ok(Json(
        s.picks
            .lock()
            .map_err(|_| anyhow::anyhow!("Picking unavailable"))?
            .pick(request, scene)?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LabelSave {
    token: String,
    text: String,
    #[serde(default)]
    details: spatial_home::notes::NoteDetails,
}
async fn label_save(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<LabelSave>,
) -> ApiResult {
    local_write(&headers)?;
    let mut picks = s
        .picks
        .lock()
        .map_err(|_| anyhow::anyhow!("Picking unavailable"))?;
    let contact = picks.contact(&request.token)?;
    if let Some(scene)=spatial_home::local_scenes::frame_scene_id(&contact.anchor.frame) {
        let editor=s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene annotations unavailable"))?.get(scene).cloned().ok_or_else(||anyhow::anyhow!("Scene annotations unavailable"))?;
        let note=editor.create(spatial_home::notes::TextNote{id:uuid::Uuid::new_v4().to_string(),text:request.text.trim().into(),details:request.details,position:contact.position,anchor_portal:String::new(),anchor_robot:None,height_m:0.,anchor_surface:Some(contact.anchor)})?;
        picks.consume(&request.token);return Ok(Json(json!(note)));
    }
    let mut store = lock(&s)?;
    let note = spatial_home::notes::add_surface_detailed(
        &mut store,
        request.text,
        contact,
        request.details,
    )?;
    picks.consume(&request.token);
    s.changes.send_replace(());
    Ok(Json(json!(note)))
}
async fn vendor(axum::extract::Path(name): axum::extract::Path<String>) -> Response {
    let path = match name.as_str() {
        "three.module.js" => "node_modules/three/build/three.module.js",
        "three.core.js" => "node_modules/three/build/three.core.js",
        "OrbitControls.js" => "node_modules/three/examples/jsm/controls/OrbitControls.js",
        "GLTFLoader.js" => "node_modules/three/examples/jsm/loaders/GLTFLoader.js",
        "DRACOLoader.js" => "node_modules/three/examples/jsm/loaders/DRACOLoader.js",
        "BufferGeometryUtils.js" => "node_modules/three/examples/jsm/utils/BufferGeometryUtils.js",
        "draco_wasm_wrapper.js" => "node_modules/three/examples/jsm/libs/draco/draco_wasm_wrapper.js",
        "draco_decoder.js" => "node_modules/three/examples/jsm/libs/draco/draco_decoder.js",
        "draco_decoder.wasm" => "node_modules/three/examples/jsm/libs/draco/draco_decoder.wasm",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let content_type = if name.ends_with(".wasm") {
        "application/wasm"
    } else {
        "text/javascript"
    };
    match std::fs::read(path) {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type)], bytes).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Run npm ci to install local viewer assets",
        )
            .into_response(),
    }
}
/// Bracketbot URDF and its Draco GLBs, read from web/robot/ like the vendor files.
async fn robot_asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    let content_type = match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("urdf") => "application/xml",
        Some("json") => "application/json",
        Some("glb") => "model/gltf-binary",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let safe = path
        .split('/')
        .all(|part| !part.is_empty() && part != ".." && !part.starts_with('.'));
    if !safe {
        return StatusCode::NOT_FOUND.into_response();
    }
    match std::fs::read(std::path::Path::new("web/robot").join(&path)) {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type)], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
/// Resolve a chat pointing target from current scene state, declare its frame,
/// and ask the robot through the `point_at` operable it advertises.
async fn point_robot_at(s:&Shared,key:&str,frame:&str,args:Value)->anyhow::Result<Value> {

                // Home resolves the target from current scene state and declares its frame;
                // the robot then decides, through its own calibration, whether it can point.
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Target {kind:String,#[serde(default)] id:Option<String>,#[serde(default)] position:Option<[f64;3]>}
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Point {#[serde(default)] robot:Option<String>,target:Target}
                let a:Point=serde_json::from_value(args)?;
                let network=spatial_home::peer_aliases::network(s.peer.borrow().clone())?;
                let candidate=spatial_home::robot_pointing::select(&network,a.robot.as_deref())?;
                let id=a.target.id.clone().filter(|v|!v.is_empty());
                let need_id=||id.clone().ok_or_else(||anyhow::anyhow!("target.id is required for kind {}",a.target.kind));
                let label_of=|peer:&str|network["peers"].as_array().and_then(|peers|peers.iter().find(|p|p["peer_id"]==peer)).and_then(|p|p["alias"].as_str().or(p["name"].as_str()).or(p["reported_name"].as_str())).unwrap_or(peer).to_owned();
                let (position,label):([f64;3],Option<String>)=match a.target.kind.as_str() {
                    "note"=>{let note_id=need_id()?;
                        let note=if key=="canonical"{spatial_home::notes::from_registries(&lock(s).map_err(|e|e.0)?.built.registries,frame)?.items.into_iter().find(|n|n.id==note_id).ok_or_else(||anyhow::anyhow!("Note not found in selected scene"))?}else{
                            s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene annotations unavailable"))?.get(key).cloned().ok_or_else(||anyhow::anyhow!("Scene annotations unavailable"))?.get_note(&note_id)?};
                        (note.position,Some(if note.details.title.is_empty(){note.text}else{note.details.title}))},
                    "volume"=>{let volume_id=need_id()?;let volume=s.local_scenes.volumes.get(key,frame)?.items.into_iter().find(|v|v.id==volume_id).ok_or_else(||anyhow::anyhow!("Volume not found in selected scene"))?;(volume.center,Some(volume.title))},
                    "peer"=>{let peer_id=need_id()?;
                        // Same registry-backed live pose resolver as get_peer_pose and the viewer.
                        let raw=s.peer.borrow().clone();
                        let pose=spatial_home::peer_locations::get(&s.previews,&raw,json!({"peer_id":peer_id}),frame).await
                            .map_err(|e|anyhow::anyhow!("No current pose for {} in the selected scene: {e}",label_of(&peer_id)))?;
                        (serde_json::from_value(pose["location"]["position"].clone())?,Some(label_of(&peer_id)))},
                    "position"=>(a.target.position.ok_or_else(||anyhow::anyhow!("target.position is required for kind position"))?,None),
                    _=>anyhow::bail!("target.kind must be note, volume, peer or position"),
                };
                let home_peer=s.peer.borrow()["connection"]["peer_id"].as_str().map(str::to_owned).ok_or_else(||anyhow::anyhow!("Home P2P peer is not ready"))?;
                let scene_frame=spatial_home::conventions::registry(&home_peer,frame)?;
                anyhow::ensure!(position.iter().all(|v|v.is_finite()),"Target position must be finite");
                let instruction=spatial_home::point_at::PointAt{request_id:uuid::Uuid::new_v4().to_string(),
                    frame:spatial_home::compact_pose::FrameRef::from(&scene_frame),point:position,arm:Default::default()};
                let robot=label_of(&candidate.peer_id);
                let (reply,receive)=tokio::sync::oneshot::channel();
                s.pointing.try_send(spatial_home::robot_pointing::Invoke{candidate:candidate.clone(),instruction,reply})
                    .map_err(|_|anyhow::anyhow!("Home P2P is not running, or another robot request is pending"))?;
                let accepted=receive.await.map_err(|_|anyhow::anyhow!("Robot pointing request was dropped"))??;
                let target=label.map(|l|format!("\"{l}\"")).unwrap_or_else(||"the point".into());
                let side=if accepted.bearing_deg>=0.{"left"}else{"right"};
                let motion=if accepted.executes{format!("Its {} arm is pointing for {:.0} s.",format!("{:?}",accepted.arm).to_lowercase(),accepted.hold_s)}else{"The arm did not move: arm motion is not enabled on the robot.".to_owned()};
                Ok(json!({"summary":format!("{robot} accepted {target}: {:.2} m away, {:.0}\u{b0} to its {side}, {:.0}\u{b0} elevation. {motion}",
                    accepted.distance_m,accepted.bearing_deg.abs(),accepted.elevation_deg),
                    "robot_peer_id":candidate.peer_id,"result":accepted}))
}
async fn chat_message(State(s):State<Shared>,headers:HeaderMap,Json(request):Json<spatial_home::chatbot::Request>)->ApiResult {
    local_write(&headers)?;
    let (key,frame)=volume_context(&s,&request.scene)?;
    let scene=request.scene.clone();
    let viewer=request.viewer.clone();
    let latest=request.messages.last().map(|m|m.content.clone()).unwrap_or_default();
    let note_view=if scene.starts_with("local:"){s.local_scenes.view(&key)?}else{serde_json::to_value(spatial_home::notes::from_registries(&lock(&s)?.built.registries,&frame)?).map_err(anyhow::Error::from)?};
    let named_notes=note_view["notes"].as_array().or_else(||note_view["items"].as_array()).map(|items|items.iter().filter_map(|n|{
        let title=n["details"]["title"].as_str().filter(|s|!s.is_empty()).or_else(||n["text"].as_str())?;
        spatial_home::scene_tools::mentions_name(&latest,title).then(||title.to_string())
    }).collect::<Vec<_>>()).unwrap_or_default();
    let tool_state=std::sync::Arc::new(std::sync::Mutex::new((std::collections::BTreeSet::<String>::new(),Vec::<[f64;3]>::new(),Vec::<[f64;3]>::new())));
    let result=spatial_home::chatbot::chat_with_async_tools(request,|name,mut args| {
        let s=s.clone();let scene=scene.clone();let key=key.clone();let frame=frame.clone();let viewer=viewer.clone();let latest=latest.clone();let named_notes=named_notes.clone();let tool_state=tool_state.clone();
        async move {
        let network=s.peer.borrow().clone();
        if name=="get_peer_pose" {return spatial_home::peer_locations::get(&s.previews,&network,args,&frame).await;}
        if name=="point_robot_at" {return point_robot_at(&s,&key,&frame,args).await;}
        let ray=if name=="raycast_peer_camera"{Some(spatial_home::chat_camera::geometry(&s.previews,&network,args.clone(),&frame).await?)}else{None};
        let reference=args["reference_id"].as_str().unwrap_or("");
        let peer_id=if name=="point_relative_to_peer"{args["peer_id"].as_str()}else if name=="create_note_relative_to_view"&&(args["reference_kind"]=="peer"||reference.starts_with("phone-name:")||reference.starts_with("robot-name:")){Some(reference.strip_prefix("phone-name:").or_else(||reference.strip_prefix("robot-name:")).unwrap_or(reference))}else{None};
        let peer_pose=if let Some(id)=peer_id {Some(spatial_home::peer_locations::read_peer(&s.previews,&network,id,&frame).await?)}else{None};
        let mut state=tool_state.lock().map_err(|_|anyhow::anyhow!("Chat tool state unavailable"))?;
        let (fresh_volumes,geometry_points,camera_hits)=&mut *state;
        #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Get {id:String}
        #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Edit {id:String,expected_description:String,description:String}
        let name=name.as_str();
        match name {
            "raycast_peer_camera"=>{
                let a:spatial_home::chat_camera::Query=serde_json::from_value(args)?;
                let view=if scene.starts_with("local:"){s.local_scenes.view(&key)?}else{serde_json::to_value(&lock(&s).map_err(|e|e.0)?.built.view)?};
                let ray=ray.ok_or_else(||anyhow::anyhow!("Camera ray unavailable"))?;
                let voxels:Vec<spatial_home::raycast::Voxel>=serde_json::from_value(view["voxels"].clone())?;
                let size=view["voxel_size"].as_f64().ok_or_else(||anyhow::anyhow!("Scene voxel geometry unavailable"))?;
                let hit=spatial_home::mobile_notes::intersection(&voxels,size,serde_json::from_value(ray["origin"].clone())?,serde_json::from_value(ray["direction"].clone())?)?;
                if let Some((hit,floor))=hit {geometry_points.push(hit.position);camera_hits.push(hit.position);Ok(json!({"frame":frame,"peer_id":a.peer_id,"position":hit.position,"hit":{"kind":if floor{"floor"}else{"voxel"},"distance_m":hit.distance},"ray":ray}))}
                else{Ok(json!({"frame":frame,"peer_id":a.peer_id,"hit":null,"ray":ray,"detail":"No occupied voxel or floor intersection within 40 meters; do not create a note without a hit"}))}
            },
            "point_relative_to_peer"=>{
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct PeerPoint {peer_id:String,direction:String,distance_m:f64,#[serde(default)]elevation_y:Option<f64>}
                let a:PeerPoint=serde_json::from_value(args)?;
                let position=spatial_home::peer_locations::relative(peer_pose.as_ref().ok_or_else(||anyhow::anyhow!("Live peer pose unavailable"))?,&frame,&a.direction,a.distance_m)?;
                let position=spatial_home::scene_geometry::placement_elevation(position,a.elevation_y,&latest)?;
                geometry_points.push(position);Ok(json!({"frame":frame,"peer_id":a.peer_id,"position":position}))
            },
            "get_volume_corners"|"point_inside_volume"|"offset_point"=>{
                let view=viewer.as_ref().ok_or_else(||anyhow::anyhow!("Captured viewer context missing; refresh Home"))?;
                anyhow::ensure!(view.frame==frame,"Viewer belongs to another scene");
                let value=if name=="offset_point"{let a:spatial_home::scene_geometry::Offset=serde_json::from_value(args)?;spatial_home::scene_geometry::offset_point(a.position,view,&a.direction,a.distance_m,&a.frame,a.ground,&frame)?}else{
                    let id=args["volume_id"].as_str().ok_or_else(||anyhow::anyhow!("Missing volume_id"))?;
                    let volume=s.local_scenes.volumes.get(&key,&frame)?.items.into_iter().find(|v|v.id==id).ok_or_else(||anyhow::anyhow!("Volume not found in selected scene"))?;
                    if name=="get_volume_corners"{let _:spatial_home::scene_geometry::Corners=serde_json::from_value(args)?;spatial_home::scene_geometry::get_volume_corners(&volume,view,&frame)?}else{let a:spatial_home::scene_geometry::Inside=serde_json::from_value(args)?;spatial_home::scene_geometry::point_inside_volume(&volume,view,&a.corner,a.distance_m,&a.mode,&frame)?}
                };
                if let Ok(point)=serde_json::from_value::<[f64;3]>(value["position"].clone()){geometry_points.push(point);}
                if let Some(corners)=value["corners"].as_array(){for corner in corners{geometry_points.push(serde_json::from_value(corner["position"].clone())?);}}
                Ok(value)
            },
            "create_note_at_volume_corner"=>{
                anyhow::ensure!(!crate::spatial_move_request(&latest),"Use read-only geometry tools then move_note for an existing note; never create a duplicate");
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Corner {name:String,description:String,volume_id:String,corner:String}
                let a:Corner=serde_json::from_value(args)?;
                if latest.to_lowercase().contains("far corner"){anyhow::ensure!(a.corner=="far","The user requested the far corner, not the near corner");}
                if latest.to_lowercase().contains("near corner"){anyhow::ensure!(a.corner=="near","The user requested the near corner, not the far corner");}
                let v=viewer.as_ref().ok_or_else(||anyhow::anyhow!("Viewer context missing; refresh Home"))?;
                anyhow::ensure!(v.frame==frame&&v.mode=="3d","Far/near corner needs the selected scene's 3D viewpoint");
                let volume=s.local_scenes.volumes.get(&key,&frame)?.items.into_iter().find(|v|v.id==a.volume_id).ok_or_else(||anyhow::anyhow!("Reference volume not found"))?;
                let position=spatial_home::scene_tools::volume_corner(&volume,v.camera_position.ok_or_else(||anyhow::anyhow!("Camera position missing; refresh Home"))?,&a.corner)?;
                let edit=spatial_home::scene_tools::NoteEdit::Create{name:a.name,description:a.description,position};
                if key=="canonical"{let mut store=lock(&s).map_err(|e|e.0)?;let receipt=spatial_home::notes::tool_edit(&mut store,edit)?;s.changes.send_replace(());Ok(receipt)}else{s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene unavailable"))?.get(&key).cloned().ok_or_else(||anyhow::anyhow!("Scene unavailable"))?.tool_edit(edit)}
            },
            "inspect_catalog"=>spatial_home::catalog_chat::inspect(&spatial_home::peer_aliases::network(s.peer.borrow().clone())?,args),
            "open_camera_pip"=>spatial_home::catalog_chat::camera_action(&s.peer.borrow(),args),
            "move_camera"=>spatial_home::view_camera::action(args,&scene,&frame,viewer.as_ref()),
            "inspect_sensor_frames"=>spatial_home::view_camera::inspect_frames(&s.peer.borrow(),args),
            "attach_camera"=>spatial_home::view_camera::attach_action(&s.peer.borrow(),args,&scene,&frame,viewer.as_ref()),
            "detach_camera"=>{anyhow::ensure!(args==json!({}),"detach_camera takes no arguments");Ok(json!({"type":"detach_camera","scene":scene,"frame":frame}))},
            "set_peer_alias"=>peer_alias_edit(&s,args),
            "create_volume"=>{
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Create {name:String,#[serde(default)]description:String,center:Option<[f64;3]>,size:Option<[f64;3]>,yaw:Option<f64>}
                let a:Create=serde_json::from_value(args)?;
                let receipt=s.local_scenes.volumes.create_volume(&key,&frame,&a.name,&a.description,a.center,a.size,a.yaw)?;
                if key=="canonical"{lock(&s).map_err(|e|e.0)?.built.registries["semantic_volumes"]=json!(s.local_scenes.volumes.get(&key,&frame)?);s.changes.send_replace(());}
                Ok(receipt)
            },
            "create_note_relative_to_view"=>{
                anyhow::ensure!(!["raycast","looking","pixel"].iter().any(|word|spatial_home::scene_tools::mentions_name(&latest,word)),"Use raycast_peer_camera then create_note for camera surface placement");
                anyhow::ensure!(!latest.to_lowercase().contains("corner"),"Use create_note_at_volume_corner for corner placement, not a center offset");
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Relative {name:String,description:String,reference_id:String,reference_kind:String,direction:String,distance_m:f64,#[serde(default)]elevation_y:Option<f64>}
                let mut a:Relative=serde_json::from_value(args)?;
                if let Some(peer)=a.reference_id.strip_prefix("phone-name:").or_else(||a.reference_id.strip_prefix("robot-name:")) {a.reference_id=peer.to_string();a.reference_kind="peer".into();}
                let instruction=latest.to_lowercase();
                if instruction.contains("behind") {anyhow::ensure!(a.direction=="behind","The user requested behind, not a sideways placement");}
                if instruction.contains("in front of") {anyhow::ensure!(a.direction=="in_front","The user requested in front, not a sideways placement");}
                let v=viewer.as_ref().ok_or_else(||anyhow::anyhow!("Viewer context missing; refresh Home before making a view-relative request"))?;
                anyhow::ensure!(v.frame==frame,"Viewer belongs to another scene frame");

                let origin:[f64;3]=match a.reference_kind.as_str(){
                    "peer"=>serde_json::from_value(peer_pose.as_ref().ok_or_else(||anyhow::anyhow!("Live peer pose unavailable"))?["home_pose"]["tracking"]["pose"]["translation"].clone())?,
                    "volume"=>s.local_scenes.volumes.get(&key,&frame)?.items.into_iter().find(|v|v.id==a.reference_id).ok_or_else(||anyhow::anyhow!("Reference volume not found"))?.center,
                    "note"=>{
                        let note=if key=="canonical"{spatial_home::notes::from_registries(&lock(&s).map_err(|e|e.0)?.built.registries,&frame)?.items.into_iter().find(|n|n.id==a.reference_id).ok_or_else(||anyhow::anyhow!("Reference note not found"))?}else{s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene unavailable"))?.get(&key).cloned().ok_or_else(||anyhow::anyhow!("Scene unavailable"))?.get_note(&a.reference_id)?};note.position
                    },
                    "anchor"=>{let view=if scene.starts_with("local:"){s.local_scenes.view(&key)?}else{serde_json::to_value(&lock(&s).map_err(|e|e.0)?.built.view)?};let portal=view["portals"].as_array().and_then(|p|p.iter().find(|p|p["anchor_id"]==a.reference_id)).ok_or_else(||anyhow::anyhow!("Reference anchor not found"))?;serde_json::from_value(portal["pose_in_map"]["translation"].clone())?},
                    _=>anyhow::bail!("Invalid reference kind")
                };
                let explicit_viewer=["from my perspective","from my viewpoint","from my point of view","from the viewer"].iter().any(|s|instruction.contains(s));
                let position=if let Some(pose)=peer_pose.as_ref().filter(|_|!explicit_viewer){
                    spatial_home::peer_locations::relative(pose,&frame,&a.direction,a.distance_m)?
                }else if matches!(a.direction.as_str(),"behind"|"in_front") {
                    anyhow::ensure!(v.mode=="3d","Behind/front is ambiguous in 2D mode; switch to 3D");
                    spatial_home::scene_tools::view_relative_depth(origin,v.camera_position.ok_or_else(||anyhow::anyhow!("Camera position missing; refresh the viewer"))?,&a.direction,a.distance_m)?
                }else{spatial_home::scene_tools::view_relative_position(origin,v.screen_right,&a.direction,a.distance_m)?};
                let position=spatial_home::scene_geometry::placement_elevation(position,a.elevation_y,&latest)?;
                let edit=spatial_home::scene_tools::NoteEdit::Create{name:a.name,description:a.description,position};
                if key=="canonical"{let mut store=lock(&s).map_err(|e|e.0)?;let receipt=spatial_home::notes::tool_edit(&mut store,edit)?;s.changes.send_replace(());Ok(receipt)}else{s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene unavailable"))?.get(&key).cloned().ok_or_else(||anyhow::anyhow!("Scene unavailable"))?.tool_edit(edit)}
            },
            "clear_voxels"=>{
                anyhow::ensure!(args.as_object().is_some_and(|a|a.is_empty()),"Unexpected tool arguments");
                anyhow::ensure!(scene.starts_with("local:"),"Only Home-owned local scene voxels can be cleared");
                let words=latest.to_lowercase().split(|c:char|!c.is_alphanumeric()).filter(|w|!w.is_empty()).map(str::to_owned).collect::<Vec<_>>();
                anyhow::ensure!(!words.iter().any(|w|matches!(w.as_str(),"not"|"don"|"never"|"without")),"The request contains a restriction on clearing; ask for an unambiguous explicit clear request");
                anyhow::ensure!(words.iter().any(|w|matches!(w.as_str(),"clear"|"reset"|"delete"|"remove"|"wipe")) && words.iter().any(|w|matches!(w.as_str(),"voxel"|"voxels"|"map")),"Clearing voxels requires an explicit user request to clear the voxel map");
                let editor=s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene unavailable"))?.get(&key).cloned().ok_or_else(||anyhow::anyhow!("Scene unavailable"))?;
                editor.clear_voxels()
            },
            "inspect_scenegraph"=>{
                anyhow::ensure!(args.as_object().is_some_and(|a|a.is_empty()),"Unexpected tool arguments");
                let view=if scene.starts_with("local:"){s.local_scenes.view(&key)?}else{serde_json::to_value(&lock(&s).map_err(|e|e.0)?.built.view)?};
                let notes=view["notes"].as_array().map(|n|n.iter().map(|n|json!({"authored":n["anchor_robot"].is_null()&&!n["id"].as_str().unwrap_or("").starts_with("robot-name:")&&!n["id"].as_str().unwrap_or("").starts_with("phone-name:"),"id":n["id"],"text":n["text"],"position":n["position"],"description":n["details"]["description"]})).collect::<Vec<_>>()).unwrap_or_default();
                let network=spatial_home::peer_aliases::network(s.peer.borrow().clone())?;
                let peers=network["peers"].as_array().map(|peers|peers.iter().map(|p|{
                    let peer=json!({"peer_id":p["peer_id"],"name":p["name"],"alias":p["alias"],"reported_name":p["reported_name"],"status":p["status"]});
                    peer
                }).collect::<Vec<_>>()).unwrap_or_default();
                Ok(json!({"scene":scene,"frame":frame,"notes":notes,"peers":peers,"peer_aliases":network["aliases"],"anchors":view["portals"],"landmarks":view["landmarks"],"semantic_volumes":s.local_scenes.volumes.get(&key,&frame)?}))
            },
            "delete_volume"=>{
                anyhow::ensure!(!spatial_home::scene_tools::requests_all_volume_deletion(&latest),"The user requested ALL volumes. Use delete_all_volumes, not a single deletion");
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Delete {id:String,expected_name:String}
                let a:Delete=serde_json::from_value(args)?;
                let instruction=latest.to_lowercase();
                let words=instruction.split(|c:char|!c.is_alphanumeric()).collect::<Vec<_>>();
                anyhow::ensure!(words.iter().any(|w|matches!(*w,"delete"|"remove"|"erase"))&&!words.iter().any(|w|matches!(*w,"not"|"don"|"never"|"without")),"Volume deletion requires an explicit, unambiguous user deletion request");
                anyhow::ensure!(fresh_volumes.contains(&a.id),"Call get_semantic_volume in the current turn before deleting");
                anyhow::ensure!(named_notes.is_empty()||spatial_home::scene_tools::mentions_name(&latest,&a.expected_name),"Do not substitute a volume for the requested note");
                let receipt=s.local_scenes.volumes.delete_volume(&key,&frame,&a.id,&a.expected_name)?;
                fresh_volumes.remove(&a.id);
                if key=="canonical"{lock(&s).map_err(|e|e.0)?.built.registries["semantic_volumes"]=json!(s.local_scenes.volumes.get(&key,&frame)?);s.changes.send_replace(());}
                Ok(receipt)
            },
            "get_semantic_volume"=>{let a:Get=serde_json::from_value(args)?;let volume=s.local_scenes.volumes.get(&key,&frame)?.items.into_iter().find(|v|v.id==a.id).ok_or_else(||anyhow::anyhow!("Volume not found in selected scene"))?;fresh_volumes.insert(a.id);{let mut value=json!(volume);value["bottom_center"]=json!([volume.center[0],volume.center[1]-volume.size[1]/2.,volume.center[2]]);Ok(value)}},
            "delete_all_volumes"=>{
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct All {expected_ids:Vec<String>}
                let a:All=serde_json::from_value(args)?;
                anyhow::ensure!(spatial_home::scene_tools::requests_all_volume_deletion(&latest),"Bulk deletion requires an explicit request to delete all volumes in this scene");
                let receipt=s.local_scenes.volumes.delete_all_volumes(&key,&frame,&a.expected_ids)?;
                fresh_volumes.clear();
                if key=="canonical"{lock(&s).map_err(|e|e.0)?.built.registries["semantic_volumes"]=json!(s.local_scenes.volumes.get(&key,&frame)?);s.changes.send_replace(());}
                Ok(receipt)
            },
            "move_volume"|"resize_volume"|"rename_volume"|"set_volume_length"|"place_volume_relative_to_floor"=>{
                let edit:spatial_home::scene_tools::VolumeEdit=spatial_home::scene_tools::decode(name,args)?;
                let target=s.local_scenes.volumes.get(&key,&frame)?.items.into_iter().find(|v|v.id==edit.id()).ok_or_else(||anyhow::anyhow!("Volume not found"))?;
                anyhow::ensure!(named_notes.is_empty()||spatial_home::scene_tools::mentions_name(&latest,&target.title),"The request names note(s) {}, not volume {}. Do not substitute a volume for a note. Ask which volume is intended for resizing; moving a note uses move_note.",named_notes.join(", "),target.title);
                anyhow::ensure!(fresh_volumes.contains(edit.id()),"Call get_semantic_volume for this ID in the current turn before editing. Old chat coordinates are not current scene state.");
                let receipt=s.local_scenes.volumes.tool_edit(&key,&frame,edit)?;
                fresh_volumes.remove(&target.id);
                if key=="canonical" {lock(&s).map_err(|e|e.0)?.built.registries["semantic_volumes"]=json!(s.local_scenes.volumes.get(&key,&frame)?);s.changes.send_replace(());}
                Ok(receipt)
            },
            "get_note"=>{let a:Get=serde_json::from_value(args)?;
                let note=if key=="canonical"{spatial_home::notes::from_registries(&lock(&s).map_err(|e|e.0)?.built.registries,&frame)?.items.into_iter().find(|n|n.id==a.id).ok_or_else(||anyhow::anyhow!("Note not found"))?}else{
                    s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene annotations unavailable"))?.get(&key).cloned().ok_or_else(||anyhow::anyhow!("Scene annotations unavailable"))?.get_note(&a.id)?
                };
                Ok(json!({"id":note.id,"name":if note.details.title.is_empty(){note.text}else{note.details.title},"position":note.position,"description":note.details.description,"frame":frame}))
            },
            "delete_all_notes"|"create_note"|"delete_note"|"move_note"|"rename_note"|"change_note_description"=>{
                if name=="delete_all_notes"{anyhow::ensure!(spatial_home::scene_tools::requests_all_note_deletion(&latest),"Bulk deletion requires an explicit request to delete all notes in this scene");}
                if name=="delete_note"{anyhow::ensure!(!spatial_home::scene_tools::requests_all_note_deletion(&latest),"Use delete_all_notes for all notes, not a single deletion");}
                if matches!(name,"create_note"|"move_note") && ["raycast","looking","pixel"].iter().any(|word|spatial_home::scene_tools::mentions_name(&latest,word)){let point:[f64;3]=serde_json::from_value(args["position"].clone())?;args["position"]=json!(spatial_home::scene_geometry::resolve_calculated_point(point,&camera_hits)?);}
                if matches!(name,"create_note"|"move_note") && ["corner","inside","closer","toward","towards","away"].iter().any(|word|spatial_home::scene_tools::mentions_name(&latest,word)){let point:[f64;3]=serde_json::from_value(args["position"].clone())?;args["position"]=json!(spatial_home::scene_geometry::resolve_calculated_point(point,&geometry_points)?);}
                let edit=spatial_home::scene_tools::decode(name,args)?;
                let receipt=if key=="canonical"{
                    let mut store=lock(&s).map_err(|e|e.0)?;
                    let receipt=spatial_home::notes::tool_edit(&mut store,edit)?;s.changes.send_replace(());receipt
                }else{
                    let editor=s.local_scenes.note_editors.lock().map_err(|_|anyhow::anyhow!("Scene annotations unavailable"))?.get(&key).cloned().ok_or_else(||anyhow::anyhow!("Scene annotations unavailable"))?;editor.tool_edit(edit)?
                };Ok(receipt)
            },
            "update_volume_description"=>{let a:Edit=serde_json::from_value(args)?;anyhow::ensure!(fresh_volumes.contains(&a.id),"Call get_semantic_volume for this ID in the current turn before editing.");let receipt=s.local_scenes.volumes.update_description(&key,&frame,&a.id,&a.expected_description,&a.description)?;
                fresh_volumes.remove(&a.id);
                if key=="canonical" {lock(&s).map_err(|e|e.0)?.built.registries["semantic_volumes"]=json!(s.local_scenes.volumes.get(&key,&frame)?);s.changes.send_replace(());}
                Ok(receipt)
            },
            _=>anyhow::bail!("Unknown scene tool"),
        }
        }
    },|args| {
        let network=s.peer.borrow().clone();let tx=s.previews.clone();
        async move {spatial_home::camera_vision::capture(&tx,&network,args).await}
    }).await?;
    Ok(Json(result))
}
fn spatial_move_request(text:&str)->bool{["move","reposition","relocate"].iter().any(|word|spatial_home::scene_tools::mentions_name(text,word))}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let simulate = std::env::var("SPATIAL_SIMULATED_ROBOT").as_deref() == Ok("1");
    anyhow::ensure!(
        !simulate || std::env::var_os("SPATIAL_PEER_CONFIG").is_none(),
        "Simulation uses isolated loopback peers; unset SPATIAL_PEER_CONFIG first"
    );
    let demo = std::env::var("SPATIAL_DEMO").as_deref() == Ok("1");
    let data = std::env::var_os("SPATIAL_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(if simulate {
                "data/simulated-robot/home.json"
            } else if demo {
                "data/home.json"
            } else {
                "data/home-portals.json"
            })
        });
    let parent = data
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let lock_path = data.with_extension("lock");
    let fiducials = Mutex::new(spatial_home::fiducials::FiducialStore::load(
        spatial_home::fiducials::registry_path(&data),
    )?);
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock_file
        .try_lock()
        .map_err(|e| anyhow::anyhow!("another server owns this journal: {e}"))?;
    let mut store = if demo {
        Store::open(data)?
    } else {
        Store::open_canonical(data)?
    };
    let local_scenes = Arc::new(spatial_home::local_scenes::LocalScenes::load(
        &lock_path.with_extension("scenes"),
    )?);
    spatial_home::notes::initialize(&mut store)?;
    local_scenes.volumes.import_notes("canonical",spatial_home::model::FRAME,
        &spatial_home::notes::from_registries(&store.built.registries,spatial_home::model::FRAME)?.volumes)?;
    store.built.registries["semantic_volumes"] = json!(local_scenes.volumes.get("canonical", spatial_home::model::FRAME)?);
    store.built.registries["fiducials"] = {
        let fiducials = fiducials
            .lock()
            .map_err(|_| anyhow::anyhow!("fiducial registry unavailable"))?;
        spatial_home::fiducials::registry_view(&fiducials)
    };
    let store = Arc::new(Mutex::new(store));
    let (changes, change_rx) = tokio::sync::watch::channel(());
    let (peer_tx, peer_rx) =
        tokio::sync::watch::channel(json!({"enabled":false,"status":"Disabled"}));
    let (runner_tx, runner_rx) =
        tokio::sync::watch::channel(spatial_home::runner_pose::RunnerState::default());
    let (mapping_tx, mapping_rx) = tokio::sync::watch::channel(
        json!({"status":"waiting","detail":"Waiting for calibrated runner LiDAR"}),
    );
    let (remote_tx, remote_rx) =
        tokio::sync::watch::channel(spatial_home::scene_mirror::RemoteScene::default());
    let (home_live_tx, home_live_rx) =
        tokio::sync::watch::channel(spatial_home::scene_mirror::RemoteScene::default());
    let (preview_tx, preview_rx) = tokio::sync::mpsc::channel(4);
    let (control_tx, control_rx) = tokio::sync::mpsc::channel(4);
    let (pointing_tx, pointing_rx) = tokio::sync::mpsc::channel(4);
    let occupancy = spatial_home::robot_occupancy::Cache::default();
    let ar_frames = spatial_home::ar_preview::Frames::default();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let simulation_disconnect = tokio_util::sync::CancellationToken::new();
    // Bind the UI before starting a remote peer, so a busy UI port cannot
    // leave a discovery registration or relay reservation alive.
    let port: u16 = std::env::var("SPATIAL_PORT")
        .unwrap_or_else(|_| "4319".into())
        .parse()?;
    let bind = ui_bind_address(std::env::var("SPATIAL_BIND").ok().as_deref())?;
    let listener = tokio::net::TcpListener::bind((bind, port)).await?;
    let (simulation_task, simulation) = if simulate {
        let (tx, rx) =
            tokio::sync::watch::channel(spatial_home::simulated_robot::SimulationState::default());
        (
            Some(tokio::spawn(spatial_home::simulated_robot::run_counted(
                tx,
                shutdown.clone(),
                simulation_disconnect.clone(),
                std::env::var("SPATIAL_SIMULATED_ROBOT_COUNT")
                    .unwrap_or_else(|_| "1".into())
                    .parse()?,
            ))),
            Some(rx),
        )
    } else {
        (None, None)
    };
    let mut runners = std::collections::BTreeMap::new();
    let network = if let Some(path) = std::env::var_os("SPATIAL_PEER_CONFIG") {
        let settings = spatial_home::peer::PeerSettings::load(std::path::Path::new(&path))?;
        let running = settings.start().await?;
        if let Some(profile) = &settings.runner_scene {
            let host = profile
                .routes
                .iter()
                .find_map(|r| r.strip_prefix("/ip4/").and_then(|r| r.split('/').next()))
                .unwrap_or("primary");
            runners.insert(
                profile.peer_id.clone(),
                RunnerView {
                    name: format!("Galbot {host}"),
                    primary: true,
                    state: remote_rx.clone(),
                },
            );
        }
        let mut extras = Vec::new();
        for extra in settings.runner_scenes {
            let (tx, rx) =
                tokio::sync::watch::channel(spatial_home::scene_mirror::RemoteScene::default());
            runners.insert(
                extra.connection.peer_id.clone(),
                RunnerView {
                    name: extra.name,
                    primary: false,
                    state: rx,
                },
            );
            extras.push((extra.connection, tx));
        }
        println!("Home P2P peer: {}", running.peer.peer_id());
        Some(tokio::spawn(spatial_home::network::serve_scene(
            running,
            store.clone(),
            local_scenes.clone(),
            runners
                .iter()
                .map(|(id, r)| (id.clone(), r.name.clone()))
                .collect(),
            change_rx,
            peer_tx,
            settings.runner,
            settings.runner_scene,
            remote_tx,
            extras,
            home_live_tx,
            runner_tx,
            spatial_home::runner_voxels::MappingTarget {
                store: store.clone(),
                changes: changes.clone(),
                status: mapping_tx,
            },
            preview_rx,
            control_rx,
            pointing_rx,
            occupancy.clone(),
            ar_frames.clone(),
            shutdown.clone(),
        )))
    } else {
        None
    };
    let state = Arc::new(AppState {
        local_scenes,
        simulation_disconnect,
        simulation,
        fiducials,
        picks: Mutex::new(Default::default()),
        occupancy,
        ar_frames,
        previews: preview_tx,
        controls: control_tx,
        pointing: pointing_tx,
        store,
        changes,
        peer: peer_rx,
        runner: runner_rx,
        mapping: mapping_rx,
        remote_scene: remote_rx,
        home_live: home_live_rx,
        runners,
    });
    let app = Router::new()
        .route("/api/chat",post(chat_message))
        .route(
            "/api/simulation/disconnect",
            post(disconnect_simulated_robots),
        )
        .route("/api/simulation/{layer}", get(simulation_scene))
        .route(
            "/",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript")],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route(
            "/phone-trails.js",
            get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/phone-trails.js")) }),
        )
        .route(
            "/ar.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript")],
                    include_str!("../web/ar.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css")],
                    include_str!("../web/style.css"),
                )
            }),
        )
        .route("/vendor/{name}", get(vendor))
        .route("/robot-model.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/robot-model.js")) }))
        .route("/robot-health.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/robot-health.js")) }))
        .route("/robot/{*path}", get(robot_asset))
        .route("/brand/mckenna.svg", get(|| async { ([(header::CONTENT_TYPE, "image/svg+xml")], include_str!("../web/brand/mckenna.svg")) }))
        .route("/brand/nils-pihl.jpg", get(|| async { ([(header::CONTENT_TYPE, "image/jpeg")], include_bytes!("../web/brand/nils-pihl.jpg").as_slice()) }))
        .route("/api/sensors", get(sensor_preview))
        .route("/api/component-enabled", post(component_enabled))
        .route("/camera-frame.js", get(|| async { ([(header::CONTENT_TYPE,"text/javascript")],include_str!("../web/camera-frame.js")) }))
        .route("/pose-inspector.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/pose-inspector.js")) }))
        .route("/video-presentation.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/video-presentation.js")) }))
        .route("/compact-pose-trail.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/compact-pose-trail.js")) }))
        .route("/api/robot-occupancy", get(robot_occupancy))
        .route("/api/scene", get(scene))
        .route("/api/semantic-volumes", get(get_volumes).post(create_volume))
        .route("/api/semantic-volumes/{id}", axum::routing::patch(update_volume).delete(delete_volume))
        .route("/panel-ui.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/panel-ui.js")) }))
        .route("/semantic-volumes.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/semantic-volumes.js")) }))
        .route("/map-2d.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/map-2d.js")) }))
        .route("/crayon.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/crayon.js")) }))
        .route("/ghost-camera.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/ghost-camera.js")) }))
        .route("/note-bubble.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/note-bubble.js")) }))
        .route("/pip-window.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/pip-window.js")) }))
        .route("/view-camera.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/view-camera.js")) }))
        .route("/attached-camera.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/attached-camera.js")) }))
        .route("/api/notes", post(add_note))
        .route("/api/notes/{id}", delete(delete_note).patch(rename_note))
        .route("/api/label-pick", post(label_pick))
        .route("/api/notes/from-pick", post(label_save))
        .route("/api/home-live-scene", get(home_live_scene))
        .route("/api/home-live-snapshot", get(home_live_snapshot))
        .route("/api/home-live-status", get(home_live_status))
        .route("/api/runner-scene", get(remote_scene))
        .route("/api/runner-snapshot", get(remote_snapshot))
        .route("/api/scene-sources", get(scene_sources))
        .route("/api/local-scenes/{id}", get(local_scene))
        .route("/api/local-scenes/{id}/snapshot", get(local_snapshot))
        .route("/api/local-scenes/{id}/reset-mapping", post(reset_local_mapping))
        .route("/api/local-scenes/{id}/presence", get(local_presence))
        .route("/api/phone-local-scene", get(phone_local_scene))
        .route("/api/domain-scene", get(domain_scene))
        .route("/api/domain-snapshot", get(domain_snapshot))
        .route("/api/fixtures", get(fixture))
        .route("/api/portal-sizes", get(portal_sizes))
        .route("/api/paths", get(get_paths).post(create_path))
        .route("/api/paths/{id}", axum::routing::delete(delete_path).patch(extend_path))
        .route("/api/landmarks", get(landmark_catalog))
        .route(
            "/api/fiducials",
            get(fiducial_catalog).post(create_fiducial),
        )
        .route("/api/fiducials/{family}/{id}/marker.svg", get(fiducial_svg))
        .route(
            "/api/fiducials/{family}/{id}",
            axum::routing::patch(resize_fiducial),
        )
        .route("/api/fiducials/{family}/{id}/print", get(fiducial_print))
        .route("/api/catalog", get(catalog))
        .route("/api/peer", get(peer_status))
        .route("/api/peer-alias",post(set_peer_alias))
        .route("/api/runner-pose", get(runner_pose))
        .route("/api/mapping", get(mapping_status))
        .route("/api/home-connection", get(home_connection))
        .route("/api/local-scenes/{id}/connection", get(local_scene_connection))
        .route("/api/snapshots", get(snapshots))
        .route("/api/contributions", post(contribute))
        .route("/api/detections", post(detect))
        .route("/api/raycast", post(cast))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(state);
    println!(
        "Spatial Home: http://127.0.0.1:{port} (listening on {bind}:{port}) — SDK PR #425; P2P {}",
        if simulate {
            "isolated robot simulation (loopback only)"
        } else if network.is_some() {
            "enabled"
        } else {
            "disabled (set SPATIAL_PEER_CONFIG)"
        }
    );
    let stop = shutdown.clone();
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = stop.cancelled() => {} }
            stop.cancel();
        })
        .await;
    shutdown.cancel();
    if let Some(network) = network {
        network.await??;
    }
    if let Some(simulation) = simulation_task {
        simulation.await??;
    }
    server?;
    Ok(())
}

#[cfg(test)]
mod ui_access_tests {
    use super::*;

    #[test]
    fn lan_is_default_and_explicit_bind_is_respected() {
        assert_eq!(ui_bind_address(None).unwrap(), "0.0.0.0".parse::<std::net::IpAddr>().unwrap());
        assert_eq!(ui_bind_address(Some("127.0.0.1")).unwrap(), "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
        assert!(ui_bind_address(Some("invalid")).is_err());
    }

    fn request(host: &str, origin: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, host.parse().unwrap());
        if let Some(origin) = origin {
            headers.insert(header::ORIGIN, origin.parse().unwrap());
        }
        headers
    }

    #[test]
    fn permits_only_configured_lan_host_and_same_origin() {
        let host = "192.168.1.26:4329";
        let headers = request(host, Some("http://192.168.1.26:4329"));
        assert!(ui_write(&headers, host).is_ok());
        assert!(ui_write(&headers, "").is_err());
        assert!(ui_write(&request(host, Some("https://foreign.example")), host).is_err());
        assert!(ui_write(&request("192.168.1.26:4330", None), host).is_err());
        assert!(ui_write(&request("foreign.example:4329", None), host).is_err());
    }

    #[test]
    fn preserves_loopback_and_rejects_malformed_hosts() {
        for host in ["localhost:4329", "127.0.0.1:4329", "[::1]:4329"] {
            assert!(ui_write(&request(host, Some(&format!("http://{host}"))), "").is_ok());
        }
        for host in ["localhost.evil:4329", "localhost:bad", "", "127.0.0.1:4329@evil"] {
            assert!(ui_write(&request(host, None), "").is_err());
        }
    }
}
