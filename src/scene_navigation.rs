//! Routes across a scene's floor, asked for by any peer and answered by Home.
//! A route is a scene path: metres in the scene frame, drawn by the code that draws drawn paths.
use anyhow::{Result, anyhow, bail, ensure};
use auki_components::*;
use serde::{Deserialize, Serialize};
use std::{cmp::Reverse, collections::{BinaryHeap, HashMap}, sync::{Arc, Mutex}};
use crate::scene_paths::{Paths, Stroke};

pub const COMPONENT: &str = "scene-navigation";
/// How a route is drawn: the same line the phones drew for their own.
pub const COLOR: &str = "#ff5fa8";
pub const WIDTH_M: f64 = 0.05;
/// Route points sit just above the floor, which is Y = 0 of the scene frame.
pub const LIFT_M: f64 = 0.06;
/// Voxels from this height up to the agent's own are in its way; lower ones are floor that has been seen.
const BODY_FROM_M: f64 = 0.25;
/// Farthest a route is planned across, in cells each way.
const REACH: i64 = 320;

/// One end of a route: a point in the request's frame, or a note in the scene,
/// whose position is the one Home holds when it plans.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Place { Point([f64; 3]), Note(String) }

/// What is walking or driving the route.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Agent { pub radius_m: f64, pub height_m: f64 }
impl Default for Agent {
    /// A person.
    fn default() -> Self { Self { radius_m: 0.25, height_m: 1.9 } }
}

/// Ask for a route. `frame` is the scene frame every point is in; Home refuses any other.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RouteRequest {
    pub frame: String,
    pub from: Place,
    pub to: Place,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Agent>,
    /// `false` returns the route without saving it to the scene's paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<bool>,
}
impl ContractType for RouteRequest { const DATATYPE: &'static str = "spatial-harness.route-request/v1"; }

/// Take the caller's route off the scene's paths.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Clear { pub frame: String }
impl ContractType for Clear { const DATATYPE: &'static str = "spatial-harness.route-clear/v1"; }

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// A way round what the map shows.
    Found,
    /// Nothing known in the way: the straight line. Floor the map has not seen is not known to be clear.
    Direct,
    /// What the map shows leaves no way through.
    Unreachable,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Route {
    pub frame: String,
    pub status: Status,
    /// One segment of points on the floor, both ends included. Absent when unreachable or already there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Stroke>,
    pub length_m: f64,
    /// When Home read the map the route was planned on, under `clock`.
    pub map_timestamp_ns: u64,
    pub clock: ClockReference,
}
impl ContractType for Route { const DATATYPE: &'static str = "spatial-harness.route/v1"; }

/// Corners of a way from `from` to `to` across the floor, both ends included, round occupied
/// voxels at the agent's height, never nearer them than its radius. Floor the
/// map has seen is preferred to floor it has not.
pub fn plan(from: [f64; 3], to: [f64; 3], voxels: &[[f64; 3]], size: f64, agent: Agent) -> Result<(Status, Vec<[f64; 3]>)> {
    ensure!(size > 0.01, "Invalid voxel size");
    let (start, goal) = ([from[0], LIFT_M, from[2]], [to[0], LIFT_M, to[2]]);
    let cell = |p: [f64; 3]| ((p[0] / size).floor() as i64, (p[2] / size).floor() as i64);
    let (a, b) = (cell(start), cell(goal));
    let margin = (3. / size).ceil() as i64;
    let low = (a.0.min(b.0) - margin, a.1.min(b.1) - margin);
    let (width, height) = ((a.0 - b.0).abs() + 2 * margin + 1, (a.1 - b.1).abs() + 2 * margin + 1);
    ensure!(width <= REACH && height <= REACH, "Route is too long to plan");
    let index = |c: (i64, i64)| {
        let (x, y) = (c.0 - low.0, c.1 - low.1);
        (x >= 0 && y >= 0 && x < width && y < height).then(|| (y * width + x) as usize)
    };
    // 0 unseen, 1 seen floor, 2 near something, 3 something in the way.
    let mut grid = vec![0u8; (width * height) as usize];
    let mut blocked = Vec::new();
    for voxel in voxels {
        let Some(i) = index(cell(*voxel)) else { continue };
        if (BODY_FROM_M..=agent.height_m).contains(&voxel[1]) {
            if grid[i] != 3 { grid[i] = 3; blocked.push(cell(*voxel)); }
        } else if voxel[1] < BODY_FROM_M && grid[i] == 0 { grid[i] = 1; }
    }
    // Nearer a voxel than the agent's radius it does not fit; one cell more is passable but kept clear of.
    let radius = (agent.radius_m / size).floor() as i64;
    let around = |c: (i64, i64), reach: i64| (-reach..=reach).flat_map(move |dy| (-reach..=reach).map(move |dx| (c.0 + dx, c.1 + dy)));
    for c in &blocked {
        for near in around(*c, radius) { if let Some(i) = index(near) { grid[i] = 3; } }
    }
    for c in &blocked {
        for near in around(*c, radius + 1) { if let Some(i) = index(near) && grid[i] < 2 { grid[i] = 2; } }
    }
    // Standing in, or heading into, something on the map: those ends stay reachable.
    for end in [a, b] {
        for near in around(end, radius + 1) { if let Some(i) = index(near) && grid[i] == 3 { grid[i] = 2; } }
    }
    // A straight stretch is clear when nothing along it is near a voxel.
    let clear = |p: [f64; 3], q: [f64; 3]| {
        let steps = ((distance(p, q) / (size / 2.)).ceil() as usize).max(1);
        (1..steps.max(2)).all(|step| {
            let t = step as f64 / steps as f64;
            index(cell([p[0] + (q[0] - p[0]) * t, LIFT_M, p[2] + (q[2] - p[2]) * t])).is_some_and(|i| grid[i] < 2)
        })
    };
    if clear(start, goal) { return Ok((Status::Direct, vec![start, goal])); }
    let (first, last) = (index(a).unwrap(), index(b).unwrap());
    let w = width as usize;
    // Costs in thousandths of a cell, so the queue orders whole numbers.
    let price = |i: usize| match grid[i] { 1 => 2u64, 0 => 5, _ => 12 };
    let ahead = |i: usize| {
        let (dx, dy) = ((i % w).abs_diff(last % w) as u64, (i / w).abs_diff(last / w) as u64);
        2 * (500 * dx.max(dy) + 207 * dx.min(dy))
    };
    let mut cost = vec![u64::MAX; grid.len()];
    let mut parent = vec![usize::MAX; grid.len()];
    let mut queue = BinaryHeap::new();
    cost[first] = 0;
    queue.push(Reverse((ahead(first), first)));
    while let Some(Reverse((rank, here))) = queue.pop() {
        if here == last { break; }
        if rank > cost[here] + ahead(here) { continue; }
        let (x, y) = ((here % w) as i64, (here / w) as i64);
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= width || ny >= height { continue; }
            let next = (ny * width + nx) as usize;
            if grid[next] == 3 { continue; }
            // No squeezing diagonally between two things.
            if dx != 0 && dy != 0 && (grid[(y * width + nx) as usize] == 3 || grid[(ny * width + x) as usize] == 3) { continue; }
            let reached = cost[here] + if dx != 0 && dy != 0 { 707 } else { 500 } * price(next);
            if reached < cost[next] { cost[next] = reached; parent[next] = here; queue.push(Reverse((reached + ahead(next), next))); }
        }
    }
    if cost[last] == u64::MAX { return Ok((Status::Unreachable, vec![])); }
    let mut cells = vec![last];
    while *cells.last().unwrap() != first { cells.push(parent[*cells.last().unwrap()]); }
    cells.reverse();
    let middle = |i: usize| [((i % w) as i64 + low.0) as f64 * size + size / 2., LIFT_M, ((i / w) as i64 + low.1) as f64 * size + size / 2.];
    let mut points = vec![start];
    if cells.len() > 2 { points.extend(cells[1..cells.len() - 1].iter().map(|i| middle(*i))); }
    points.push(goal);
    let (mut corners, mut from) = (vec![points[0]], 0);
    while from < points.len() - 1 {
        let next = (from + 2..points.len()).rev().find(|far| clear(points[from], points[*far])).unwrap_or(from + 1);
        corners.push(points[next]);
        from = next;
    }
    Ok((Status::Found, corners))
}
fn distance(p: [f64; 3], q: [f64; 3]) -> f64 { (0..3).map(|i| (p[i] - q[i]).powi(2)).sum::<f64>().sqrt() }
pub fn length(route: &[[f64; 3]]) -> f64 { route.windows(2).map(|w| distance(w[0], w[1])).sum() }
/// The route's corners with points every `gap` between them, so a smoothed line stays on it round corners.
fn spaced(route: &[[f64; 3]], gap: f64) -> Vec<[f64; 3]> {
    let mut points = route[..1].to_vec();
    for w in route.windows(2) {
        let steps = (distance(w[0], w[1]) / gap).ceil().max(1.) as usize;
        points.extend((1..steps).map(|s| std::array::from_fn(|i| w[0][i] + (w[1][i] - w[0][i]) * s as f64 / steps as f64)));
        points.push(w[1]);
    }
    points
}

/// What Home knows of one scene, read when a route is asked for.
pub trait World: Send + Sync + 'static {
    /// Centres of the occupied voxels of the scene's map, in the scene frame, and their size.
    fn voxels(&self) -> Result<(Vec<[f64; 3]>, f64)>;
    /// A note's words and where it is now.
    fn note(&self, id: &str) -> Result<Option<(String, [f64; 3])>>;
    /// Add (`Some`) or remove (`None`) a path in the scene's saved paths.
    fn store(&self, id: &str, stroke: Option<Stroke>) -> Result<Paths>;
    /// Now, under the clock the component declares.
    fn now(&self) -> u64;
}

/// Plans routes for one scene and keeps at most one shared route per caller.
pub struct Navigator<W> { frame: String, clock: ClockReference, world: W, routes: Mutex<HashMap<String, String>> }
impl<W: World> Navigator<W> {
    pub fn new(frame: &str, clock: ClockReference, world: W) -> Self {
        Self { frame: frame.into(), clock, world, routes: Mutex::new(HashMap::new()) }
    }
    fn place(&self, place: &Place) -> Result<(Option<String>, [f64; 3])> {
        let (name, point) = match place {
            Place::Point(point) => (None, *point),
            Place::Note(id) => {
                let (text, point) = self.world.note(id)?.ok_or_else(|| anyhow!("Note not found in this scene"))?;
                (Some(text), point)
            }
        };
        ensure!(point.iter().all(|v| v.is_finite() && v.abs() <= 100000.), "Invalid route coordinate");
        Ok((name, point))
    }
    pub fn plan(&self, caller: &str, request: &RouteRequest) -> Result<Route> {
        ensure!(!caller.is_empty(), "Route caller is not authenticated");
        ensure!(request.frame == self.frame, "Scene frame changed; ask for the route again");
        let agent = request.agent.unwrap_or_default();
        ensure!((0.05..=2.).contains(&agent.radius_m) && (0.3..=3.).contains(&agent.height_m), "Invalid route agent");
        let ((origin, from), (name, to)) = (self.place(&request.from)?, self.place(&request.to)?);
        let map_timestamp_ns = self.world.now();
        let (voxels, size) = self.world.voxels()?;
        let (status, corners) = plan(from, to, &voxels, size, agent)?;
        let length_m = length(&corners);
        let path = (length_m > 0.05).then(|| {
            let trim = |s: &str| s.trim().to_owned();
            let title = match (origin.as_deref().map(trim), name.as_deref().map(trim)) {
                (Some(a), Some(b)) => format!("Way from {a} to {b}"),
                (None, Some(b)) => format!("Way to {b}"),
                _ => "Route".into(),
            };
            Stroke { id: uuid::Uuid::new_v4().to_string(), title: title.chars().take(120).collect(), color: COLOR.into(),
                width_m: WIDTH_M, segments: vec![spaced(&corners, 0.3)] }
        });
        if request.share != Some(false) {
            // The caller's new route replaces its last, and no way through leaves it with none.
            let mut routes = self.routes.lock().map_err(|_| anyhow!("Routes unavailable"))?;
            if let Some(old) = routes.remove(caller) { self.remove(&old)?; }
            if let Some(path) = &path {
                self.world.store(&path.id, Some(path.clone()))?;
                routes.insert(caller.into(), path.id.clone());
            }
        }
        Ok(Route { frame: self.frame.clone(), status, path, length_m, map_timestamp_ns, clock: self.clock.clone() })
    }
    /// True when the caller had a route on the scene's paths.
    pub fn clear(&self, caller: &str, request: &Clear) -> Result<bool> {
        ensure!(request.frame == self.frame, "Scene frame changed");
        let Some(old) = self.routes.lock().map_err(|_| anyhow!("Routes unavailable"))?.remove(caller) else { return Ok(false) };
        self.remove(&old)
    }
    /// Anyone may erase a path, so a route already gone is not an error.
    fn remove(&self, id: &str) -> Result<bool> {
        match self.world.store(id, None) {
            Ok(_) => Ok(true),
            Err(e) if e.to_string() == "Path not found" => Ok(false),
            Err(e) => bail!(e),
        }
    }
}

/// What a phone's app hands its core: a route to plan, or its route to take away.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Ask { Plan(RouteRequest), Clear(Clear) }
/// The app's own `id` comes back with Home's answer.
#[derive(Clone, Debug, Deserialize)]
pub struct Asked { pub id: String, pub ask: Ask }
/// Ask Home's navigation component for the paired scene. `Ok(None)` is a `clear` Home carried out.
pub async fn send(peer: &auki_sdk::AukiPeer, home: &crate::network::HomeConnection, ask: &Ask) -> Result<Option<Route>> {
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
            .ok_or_else(|| anyhow!("Home navigation component unavailable"))?.manifest.reference();
        let (request, wait) = (uuid::Uuid::new_v4().to_string(), Some(Duration::from_secs(3)));
        return Ok(match ask {
            Ask::Plan(plan) => Some(client.invoke_exact::<_, Route>(expected, route, component, "plan", "route-caller", request, wait, plan).await?.result),
            Ask::Clear(clear) => { client.invoke_exact::<_, bool>(expected, route, component, "clear", "route-caller", request, wait, clear).await?; None }
        });
    }
    bail!("Home navigation component unreachable")
}

/// Mount `scene-navigation:<id>` with its `plan` and `clear` Operables, exported for peers of the scene.
pub fn mount<W: World>(runtime: &ComponentRuntime, endpoint: &auki_component_protocol::ComponentProtocolEndpoint,
    id: &str, frame: &str, clock: ClockReference, world: W) -> Result<(Component, Operable<RouteRequest, Route>)> {
    let operable = |name: &str, instruction: &str, result: &str| OperableContract {
        name: name.into(), instruction: instruction.into(), result: result.into(), exposure: Exposure::Cluster };
    let component = runtime.component(ComponentSpec::new(format!("{COMPONENT}:{id}"))
        .operable(operable("plan", RouteRequest::DATATYPE, Route::DATATYPE))
        .operable(operable("clear", Clear::DATATYPE, "bool")))?;
    let navigator = Arc::new(Navigator::new(frame, clock, world));
    let rejected = |e: anyhow::Error| InvocationError::Rejected(e.to_string());
    // Any authenticated peer of the scene may ask, as any may draw a path.
    let planner = navigator.clone();
    let plan = component.operable("plan", |ctx| !ctx.caller_peer_id.is_empty(),
        move |ctx, request: RouteRequest| planner.plan(&ctx.caller_peer_id, &request).map_err(rejected))?;
    let clear = component.operable("clear", |ctx| !ctx.caller_peer_id.is_empty(),
        move |ctx, request: Clear| navigator.clear(&ctx.caller_peer_id, &request).map_err(rejected))?;
    component.expose()?;
    endpoint.export_operable(&plan)?;
    endpoint.export_operable(&clear)?;
    Ok((component, plan))
}
