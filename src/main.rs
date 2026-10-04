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
    picks: Mutex<spatial_home::picking::Picks>,
    occupancy: spatial_home::robot_occupancy::Cache,
    ar_frames: spatial_home::ar_preview::Frames,
    previews: tokio::sync::mpsc::Sender<spatial_home::sensor_preview::Request>,
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
    sensor: Option<String>,
    #[serde(default)]
    ar: bool,
    #[serde(default)]
    pick: bool,
}
async fn sensor_preview(
    State(s): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<PreviewQuery>,
) -> ApiResult {
    let (reply, receive) = tokio::sync::oneshot::channel();
    s.previews
        .try_send(spatial_home::sensor_preview::Request {
            sensor: q.sensor,
            ar: q.ar,
            pick: q.pick,
            reply,
        })
        .map_err(|_| anyhow::anyhow!("Sensor connection unavailable"))?;
    let mut result = tokio::time::timeout(std::time::Duration::from_secs(10), receive)
        .await
        .map_err(|_| anyhow::anyhow!("Sensor request timed out"))?
        .map_err(|_| anyhow::anyhow!("Sensor connection stopped"))??;
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
    Ok(Json(result))
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
    let mut local = vec![json!({"id":"demo","name":"Home Server"})];
    if s.home_live.borrow().scene.is_some() {
        local.push(json!({"id":"home-live","name":"Home live map"}));
    }
    status["local"] = json!(local);
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
    Ok(Json(status))
}
async fn scene(State(s): State<Shared>) -> ApiResult {
    let store = lock(&s)?;
    let mut view = serde_json::to_value(&store.built.view).map_err(anyhow::Error::from)?;
    view["notes"] = json!(
        spatial_home::notes::from_registries(&store.built.registries, spatial_home::model::FRAME)?
            .items
    );
    spatial_home::mobile_pose::append_labels(&mut view, &store.built.registries["mobile_presence"]);
    Ok(Json(view))
}
async fn delete_note(
    State(s): State<Shared>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult {
    local_write(&headers)?;
    let mut store = lock(&s)?;
    spatial_home::notes::remove(&mut store, &id)?;
    s.changes.send_replace(());
    Ok(Json(json!({"deleted":id})))
}
async fn rename_note(
    State(s): State<Shared>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(request): Json<serde_json::Value>,
) -> ApiResult {
    local_write(&headers)?;
    let mut store = lock(&s)?;
    let text = request["text"].as_str().unwrap_or_default().to_owned();
    let details = request
        .get("details")
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(anyhow::Error::from)?;
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
    Json(note): Json<NewNoteRequest>,
) -> ApiResult {
    local_write(&headers)?;
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
    Ok(Json(s.peer.borrow().clone()))
}
async fn home_connection(State(s): State<Shared>) -> ApiResult {
    let state = s.peer.borrow();
    let profile = state
        .get("connection")
        .filter(|_| state["status"] == "Ready")
        .ok_or_else(|| ApiError(anyhow::anyhow!("Home P2P peer is not ready")))?;
    Ok(Json(profile.clone()))
}
async fn snapshots(State(s): State<Shared>) -> ApiResult {
    let s = lock(&s)?;
    Ok(Json(
        json!({"domain_portals":s.built.domain_portals,"portals":s.built.portals,"voxels":s.built.voxels,"objects":s.built.view.objects,"registries":s.built.registries}),
    ))
}
// Local test control boundary: JSON only, no CORS, and reject foreign browser
// origins. This is intentionally not remote peer authentication or an SDK P2P API.
fn local_write(headers: &HeaderMap) -> Result<(), ApiError> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let allowed = host.starts_with("127.0.0.1:") || host.starts_with("localhost:");
    if !allowed {
        return Err(ApiError(anyhow::anyhow!("localhost Host required")));
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
    let live = s.home_live.borrow();
    if live.status_view()["status"] != "connected" {
        if let spatial_home::picking::PickRequest::Scene {
            frame,
            origin,
            direction,
        } = &request
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
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match std::fs::read(path) {
        Ok(bytes) => ([(header::CONTENT_TYPE, "text/javascript")], bytes).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Run npm ci to install local viewer assets",
        )
            .into_response(),
    }
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let demo = std::env::var("SPATIAL_DEMO").as_deref() == Ok("1");
    let data = std::env::var_os("SPATIAL_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(if demo {
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
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock_file
        .try_lock()
        .map_err(|e| anyhow::anyhow!("another server owns this journal: {e}"))?;
    let store = Arc::new(Mutex::new(if demo {
        Store::open(data)?
    } else {
        Store::open_canonical(data)?
    }));
    spatial_home::notes::initialize(&mut store.lock().unwrap())?;
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
    let occupancy = spatial_home::robot_occupancy::Cache::default();
    let ar_frames = spatial_home::ar_preview::Frames::default();
    let shutdown = tokio_util::sync::CancellationToken::new();
    // Bind the local UI before starting a remote peer, so a busy UI port cannot
    // leave a discovery registration or relay reservation alive.
    let port: u16 = std::env::var("SPATIAL_PORT")
        .unwrap_or_else(|_| "4319".into())
        .parse()?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
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
            occupancy.clone(),
            ar_frames.clone(),
            shutdown.clone(),
        )))
    } else {
        None
    };
    let state = Arc::new(AppState {
        picks: Mutex::new(Default::default()),
        occupancy,
        ar_frames,
        previews: preview_tx,
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
        .route("/api/sensors", get(sensor_preview))
        .route("/api/robot-occupancy", get(robot_occupancy))
        .route("/api/scene", get(scene))
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
        .route("/api/domain-scene", get(domain_scene))
        .route("/api/domain-snapshot", get(domain_snapshot))
        .route("/api/fixtures", get(fixture))
        .route("/api/portal-sizes", get(portal_sizes))
        .route("/api/catalog", get(catalog))
        .route("/api/peer", get(peer_status))
        .route("/api/runner-pose", get(runner_pose))
        .route("/api/mapping", get(mapping_status))
        .route("/api/home-connection", get(home_connection))
        .route("/api/snapshots", get(snapshots))
        .route("/api/contributions", post(contribute))
        .route("/api/detections", post(detect))
        .route("/api/raycast", post(cast))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(state);
    println!(
        "Spatial Home: http://127.0.0.1:{port} — SDK PR #425; P2P {}",
        if network.is_some() {
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
    server?;
    Ok(())
}
