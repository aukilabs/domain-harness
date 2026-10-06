//! Robot-free local Component/Product pipeline with a small inspection UI.
use anyhow::Result;
use auki_components::ComponentRuntime;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Html,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use spatial_home::{
    model,
    occupancy_fixture::{FakeVoxelSource, Scenario},
    occupancy_mapper::{Config, OccupancyMapper, OccupancyScene},
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    scenario: Scenario,
    paused: bool,
}
#[derive(Default, Serialize)]
struct Demo {
    control: Control,
    source_scenario: Option<Scenario>,
    source_publications: u64,
    occupancy: Option<OccupancyScene>,
    error: Option<String>,
}
type Shared = Arc<Mutex<Demo>>;

async fn control(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(control): Json<Control>,
) -> Result<Json<Control>, StatusCode> {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !(host.starts_with("127.0.0.1:") || host.starts_with("localhost:")) {
        return Err(StatusCode::FORBIDDEN);
    }
    if let Some(origin) = headers.get("origin")
        && origin.to_str().ok() != Some(format!("http://{host}").as_str())
    {
        return Err(StatusCode::FORBIDDEN);
    }
    state
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .control = control;
    Ok(Json(control))
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut port = 4321_u16;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                port = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing port"))?
                    .parse()?
            }
            "--help" | "-h" => {
                println!(
                    "occupancy-demo [--port 4321]\nLocal synthetic voxel Component → real occupancy mapper at 1Hz. No robot, credentials, or external services."
                );
                return Ok(());
            }
            _ => anyhow::bail!("unknown option {arg}"),
        }
    }
    let runtime = ComponentRuntime::new("occupancy-demo");
    let clock = model::clock_entry("occupancy-demo", &uuid::Uuid::new_v4().to_string());
    let mut source = FakeVoxelSource::new(&runtime, "occupancy-demo", clock.clone())?;
    let mut mapper = OccupancyMapper::mount_local(
        &runtime,
        &source.product(),
        "occupancy-demo",
        model::clock_ref(&clock),
        Config::new(0.),
    )?;
    let state = Arc::new(Mutex::new(Demo::default()));
    let app = Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../../web/occupancy-demo.html")) }),
        )
        .route(
            "/api/state",
            get(|State(s): State<Shared>| async move {
                s.lock()
                    .map(|s| Json(serde_json::to_value(&*s).unwrap()))
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
            }),
        )
        .route("/api/control", post(control))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    println!(
        "Synthetic occupancy demo: http://{} (Ctrl-C to stop)",
        listener.local_addr()?
    );
    let work = async {
        let mut publish = tokio::time::interval(Duration::from_secs(1));
        publish.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut process = tokio::time::interval(Duration::from_millis(50));
        process.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = publish.tick() => {
                    let control = state.lock().unwrap().control;
                    if !control.paused {
                        match source.publish(control.scenario) {
                            Ok(_) => { let mut s = state.lock().unwrap(); s.source_scenario=Some(control.scenario);s.source_publications+=1;s.error=None; },
                            Err(e) => { state.lock().unwrap().error=Some(e.to_string()); }
                        }
                    }
                }
                _ = process.tick() => {
                    match mapper.tick() {
                        Ok(_) => state.lock().unwrap().occupancy=Some(mapper.state().clone()),
                        Err(e) => state.lock().unwrap().error=Some(e.to_string()),
                    }
                }
            }
        }
    };
    tokio::select! {
        _ = work => {},
        result = axum::serve(listener,app) => result?,
        result = tokio::signal::ctrl_c() => result?,
    }
    Ok(())
}
