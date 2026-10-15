//! One-shot visual floor routes, using Annotator's MessageRoute policy and the
//! existing scene-path contract. This is not a robot navigation command.
use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use futures::{stream, StreamExt, TryStreamExt};
use std::{cmp::Reverse, collections::{BTreeMap, BTreeSet, BinaryHeap}, time::{Duration, Instant}};

const LIFT: f64 = 0.06;
const REACH: usize = 320;
const MAX_VOXELS: usize = 1_000_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target { Peer { id: String }, Note { id: String }, Volume { id: String }, Portal { id: String } }
impl Target {
    fn id(&self) -> &str { match self { Self::Peer{id}|Self::Note{id}|Self::Volume{id}|Self::Portal{id} => id } }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query { pub waypoints: Vec<Target> }

pub struct LiveEndpoints(BTreeMap<String, (Value, Instant)>);
impl LiveEndpoints {
    pub fn validate(&self, frame: &str) -> Result<()> {
        for (value, received) in self.0.values() {
            crate::peer_locations::location(value, frame, received.elapsed())?;
        }
        Ok(())
    }
}
#[derive(Serialize)]
pub struct Endpoint { pub name: String, pub position: [f64; 3], pub target: Target }
#[derive(Serialize)]
pub struct Planned {
    pub stroke: crate::scene_paths::Stroke,
    pub waypoints: Vec<Endpoint>,
    pub length_m: f64,
    pub crosses_unobserved_floor: bool,
}
impl Query {
    /// Resolve each distinct peer once with bounded concurrency. Receipt times
    /// prevent early results aging out while waiting for others or planning.
    pub async fn live_endpoints(&self, tx: &tokio::sync::mpsc::Sender<crate::sensor_preview::Request>, network: &Value, frame: &str) -> Result<LiveEndpoints> {
        ensure!((2..=64).contains(&self.waypoints.len()), "/path needs 2–64 waypoints");
        let ids=self.waypoints.iter().filter_map(|t|if let Target::Peer{id}=t {Some(id.clone())}else{None}).collect::<BTreeSet<_>>();
        let reads=stream::iter(ids).map(|id|async move {
            let value=crate::peer_locations::read_peer(tx,network,&id,frame).await?;
            Ok::<_,anyhow::Error>((id,(value,Instant::now())))
        }).buffer_unordered(4).try_collect();
        let endpoints=LiveEndpoints(tokio::time::timeout(Duration::from_secs(5),reads).await.map_err(|_|anyhow!("Live waypoint lookup timed out"))??);
        endpoints.validate(frame)?; Ok(endpoints)
    }

    pub fn plan(&self, view: &Value, network: &Value, frame: &str, live: &LiveEndpoints) -> Result<Planned> {
        ensure!(view["frame"] == frame, "Scene frame changed; select the endpoints again");
        live.validate(frame)?;
        ensure!((2..=64).contains(&self.waypoints.len()), "/path needs 2–64 waypoints");
        let resolve = |target: &Target| -> Result<Endpoint> {
            let id = target.id(); ensure!(!id.is_empty(), "Missing path endpoint ID");
            let find = |rows: &Value, field: &str| -> Result<Value> {
                rows.as_array().and_then(|rows| rows.iter().find(|r| r[field] == id)).cloned()
                    .ok_or_else(|| anyhow!("Path endpoint no longer exists in the selected scene"))
            };
            let (name, position) = match target {
                Target::Peer{..} => {
                    let row = find(&network["peers"], "peer_id")?;
                    let (pose, received) = live.0.get(id).ok_or_else(||anyhow!("Live path endpoint unavailable"))?;
                    let location = crate::peer_locations::location(pose, frame, received.elapsed())?;
                    let name = row["alias"].as_str().or_else(||network["aliases"][id].as_str()).or_else(||row["name"].as_str()).unwrap_or(id).to_owned();
                    (name, location["position"].clone())
                },
                Target::Note{..} => {
                    let row = find(&view["notes"], "id")?;
                    ensure!(row["anchor_robot"].is_null() && !id.starts_with("phone-name:") && !id.starts_with("robot-name:"), "Select a live #peer, not a cached peer label");
                    (row["text"].as_str().unwrap_or(id).to_owned(), row["position"].clone())
                },
                Target::Volume{..} => {
                    let row = find(&view["semantic_volumes"]["items"], "id")?;
                    (row["title"].as_str().unwrap_or(id).to_owned(), row["center"].clone())
                },
                Target::Portal{..} => {
                    let row = find(&view["portals"], "anchor_id")?;
                    (row["payload"].as_str().unwrap_or(id).to_owned(), row["pose_in_map"]["translation"].clone())
                },
            };
            let position: [f64;3] = serde_json::from_value(position)?;
            ensure!(position.iter().all(|v|v.is_finite() && v.abs() <= 100_000.), "Invalid path endpoint position");
            Ok(Endpoint { name, position, target: target.clone() })
        };
        let waypoints = self.waypoints.iter().map(resolve).collect::<Result<Vec<_>>>()?;
        let size = view["voxel_size"].as_f64().ok_or_else(||anyhow!("Scene voxel size unavailable"))?;
        let voxels = view["voxels"].as_array().ok_or_else(||anyhow!("Scene voxels unavailable"))?;
        ensure!(voxels.len() <= MAX_VOXELS, "Scene exceeds visual route voxel budget");
        let centers = voxels.iter().filter(|v|v["evidence"].as_f64().unwrap_or(1.) > 0.)
            .map(|v|serde_json::from_value(v["center"].clone())).collect::<std::result::Result<Vec<[f64;3]>, _>>()?;
        ensure!(centers.len()*(waypoints.len()-1)<=8_000_000, "Path exceeds visual route voxel-work budget");
        let mut points=Vec::new();let mut crosses_unobserved_floor=false;let mut grid_budget=1_000_000usize;
        for (index,pair) in waypoints.windows(2).enumerate() {
            let route=plan_bounded(pair[0].position,pair[1].position,&centers,size,&mut grid_budget)
                .map_err(|e|anyhow!("Leg {} ({} → {}): {e}",index+1,pair[0].name,pair[1].name))?;
            crosses_unobserved_floor|=route.crosses_unobserved_floor;
            points.extend(route.points.into_iter().skip(if index==0 {0}else{1}));
            ensure!(points.len()<=4096,"Visual route exceeds path point budget");
        }
        let length_m = points.windows(2).map(|p| (p[0][0]-p[1][0]).hypot(p[0][2]-p[1][2])).sum();
        let names=waypoints.iter().map(|p|p.name.trim()).collect::<Vec<_>>().join(" → ");
        let stroke = crate::scene_paths::Stroke {
            id: uuid::Uuid::new_v4().to_string(), title: format!("Way: {names}").chars().take(120).collect(),
            color: "#ff5fa8".into(), width_m: 0.05, segments: vec![points],
        };
        crate::scene_paths::Paths {schema:crate::scene_paths::SCHEMA.into(),frame:frame.into(),items:vec![stroke.clone()]}.validate(frame)?;
        Ok(Planned { stroke, waypoints, length_m, crosses_unobserved_floor })
    }
}

pub struct Route { pub points: Vec<[f64;3]>, pub crosses_unobserved_floor: bool }
struct Grid { size: f64, low: [i32;2], width: usize, height: usize, cells: Vec<u8>, seen: Vec<bool> }
impl Grid {
    fn cell(&self, p: [f64;3]) -> [i32;2] { [(p[0]/self.size).floor() as i32, (p[2]/self.size).floor() as i32] }
    fn index(&self, c: [i32;2]) -> Option<usize> {
        let x = c[0]-self.low[0]; let y = c[1]-self.low[1];
        (x>=0 && y>=0 && x<self.width as i32 && y<self.height as i32).then(|| y as usize*self.width+x as usize)
    }
    fn center(&self, i: usize) -> [f64;3] { [((i%self.width) as f64+self.low[0] as f64+0.5)*self.size, LIFT, ((i/self.width) as f64+self.low[1] as f64+0.5)*self.size] }
    /// Supercover grid traversal, including both side cells at diagonal corners.
    /// Unlike point sampling, this cannot skip a thin occupied corner.
    fn line(&self, a: [f64;3], b: [f64;3], mut visit: impl FnMut(usize)->bool) -> bool {
        let mut cell = self.cell(a); let end = self.cell(b);
        let start = [a[0]/self.size, a[2]/self.size];
        let d = [(b[0]-a[0])/self.size, (b[2]-a[2])/self.size];
        let step = d.map(|n| if n>0. {1} else if n<0. {-1} else {0});
        let delta = d.map(|n| if n==0. {f64::INFINITY} else {1./n.abs()});
        let mut next: [f64;2] = std::array::from_fn(|i| if step[i]==0 {f64::INFINITY} else {((cell[i]+if step[i]>0 {1}else{0}) as f64-start[i])/d[i]});
        for _ in 0..self.width+self.height+4 {
            if !self.index(cell).is_some_and(&mut visit) { return false; }
            if cell==end { return true; }
            if (next[0]-next[1]).abs()<1e-10 {
                if !self.index([cell[0]+step[0],cell[1]]).is_some_and(&mut visit)
                    || !self.index([cell[0],cell[1]+step[1]]).is_some_and(&mut visit) { return false; }
                for i in 0..2 {cell[i]+=step[i];next[i]+=delta[i];}
            } else {let i=if next[0]<next[1]{0}else{1};cell[i]+=step[i];next[i]+=delta[i];}
        }
        false
    }
}

/// Annotator's flat-floor, body-height, weighted eight-neighbour A* policy.
/// Unknown floor is allowed but reported; this is guidance, not a certified
/// drivable route. Never unblock endpoints, move them or draw through a wall
/// as a fallback when the bounded search fails.
pub fn plan(from: [f64;3], to: [f64;3], voxels: &[[f64;3]], size: f64) -> Result<Route> {
    plan_bounded(from,to,voxels,size,&mut (REACH*REACH))
}
fn plan_bounded(from: [f64;3], to: [f64;3], voxels: &[[f64;3]], size: f64, grid_budget: &mut usize) -> Result<Route> {
    ensure!(size.is_finite() && (0.02..=1.).contains(&size), "Unsupported visual route voxel size");
    ensure!(from.iter().chain(to.iter()).all(|v|v.is_finite() && v.abs()<=100_000.), "Invalid route endpoint");
    ensure!(voxels.len()<=MAX_VOXELS, "Scene exceeds visual route voxel budget");
    let start=[from[0],LIFT,from[2]];let goal=[to[0],LIFT,to[2]];
    ensure!((start[0]-goal[0]).hypot(start[2]-goal[2])>0.05, "Endpoints are already at the same floor position");
    let cell=|p:[f64;3]|[(p[0]/size).floor() as i32,(p[2]/size).floor() as i32];
    let a=cell(start);let b=cell(goal);let margin=(3./size).ceil() as i32;
    let width=(a[0]-b[0]).unsigned_abs() as usize+2*margin as usize+1;
    let height=(a[1]-b[1]).unsigned_abs() as usize+2*margin as usize+1;
    ensure!(width<=REACH && height<=REACH, "Endpoints exceed the visual route search range (320 cells per axis)");
    ensure!(*grid_budget>=width*height,"Path exceeds visual route search-work budget");*grid_budget-=width*height;
    let mut grid=Grid{size,low:[a[0].min(b[0])-margin,a[1].min(b[1])-margin],width,height,cells:vec![0;width*height],seen:vec![false;width*height]};
    for &p in voxels {
        ensure!(p.iter().all(|v|v.is_finite() && v.abs()<=100_000.), "Invalid scene voxel position");
        let c=cell(p);let Some(i)=grid.index(c) else {continue};
        if (0.25..=1.9).contains(&p[1]) {grid.cells[i]=3;}
        else if (-size..0.25).contains(&p[1]) {grid.seen[i]=true;if grid.cells[i]==0 {grid.cells[i]=1;}}
    }
    let first=grid.index(a).unwrap();let last=grid.index(b).unwrap();
    ensure!(grid.cells[first]!=3, "Path start is blocked by mapped obstacles; no path saved");
    ensure!(grid.cells[last]!=3, "Path destination is blocked by mapped obstacles; no path saved");
    let clearance=(0.25/size).round().max(1.) as i32;
    // Summed-area table bounds clearance work to O(grid cells), even on dense maps.
    let stride=width+1;let mut occupied=vec![0i32;stride*(height+1)];
    for y in 0..height {for x in 0..width {
        occupied[(y+1)*stride+x+1]=i32::from(grid.cells[y*width+x]==3)+occupied[y*stride+x+1]+occupied[(y+1)*stride+x]-occupied[y*stride+x];
    }}
    for y in 0..height {for x in 0..width {
        if grid.cells[y*width+x]>=2 {continue;}
        let x0=x.saturating_sub(clearance as usize);let y0=y.saturating_sub(clearance as usize);
        let x1=(x+clearance as usize+1).min(width);let y1=(y+clearance as usize+1).min(height);
        if occupied[y1*stride+x1]-occupied[y0*stride+x1]-occupied[y1*stride+x0]+occupied[y0*stride+x0]>0 {grid.cells[y*width+x]=2;}
    }}
    let ahead=|i:usize| {let dx=(i%width).abs_diff(last%width);let dy=(i/width).abs_diff(last/width);(20*dx.max(dy)+8*dx.min(dy)) as u32};
    let mut cost=vec![u32::MAX;grid.cells.len()];let mut parent=vec![usize::MAX;grid.cells.len()];
    let mut closed=vec![false;grid.cells.len()];let mut heap=BinaryHeap::new();
    cost[first]=0;heap.push(Reverse((ahead(first),0,first)));
    while let Some(Reverse((_,g,here)))=heap.pop() {
        if g!=cost[here] || closed[here] {continue;}if here==last {break;}closed[here]=true;
        let x=(here%width) as i32;let y=(here/width) as i32;
        for (dx,dy) in [(1,0),(-1,0),(0,1),(0,-1),(1,1),(1,-1),(-1,1),(-1,-1)] {
            let nx=x+dx;let ny=y+dy;if nx<0||ny<0||nx>=width as i32||ny>=height as i32 {continue;}
            let next=ny as usize*width+nx as usize;
            if closed[next]||grid.cells[next]==3 {continue;}
            if dx!=0&&dy!=0&&(grid.cells[y as usize*width+nx as usize]==3||grid.cells[ny as usize*width+x as usize]==3) {continue;}
            let price_at=|i:usize|match grid.cells[i] {1=>2,0=>5,_=>12};
            let mut price=price_at(next);
            if dx!=0&&dy!=0 {price=price.max(price_at(y as usize*width+nx as usize)).max(price_at(ny as usize*width+x as usize));}
            let reached=g+if dx!=0&&dy!=0 {14*price}else{10*price};
            if reached<cost[next] {cost[next]=reached;parent[next]=here;heap.push(Reverse((reached+ahead(next),reached,next)));}
        }
    }
    ensure!(cost[last]!=u32::MAX, "No floor route found within the mapped search area; no path saved");
    let mut cells=vec![last];while *cells.last().unwrap()!=first {cells.push(parent[*cells.last().unwrap()]);}cells.reverse();
    let mut points=vec![start];points.extend(cells.iter().map(|&i|grid.center(i)));points.push(goal);
    points.dedup_by(|a,b| (a[0]-b[0]).hypot(a[2]-b[2])<1e-8);
    points[0]=start;*points.last_mut().unwrap()=goal;
    // Drop collinear grid centres before bounded visibility shortcuts.
    let mut corners:Vec<[f64;3]>=Vec::new();
    for p in points {
        while corners.len()>=2 {
            let a=corners[corners.len()-2];let b=corners[corners.len()-1];
            let u=[b[0]-a[0],b[2]-a[2]];let v=[p[0]-b[0],p[2]-b[2]];
            if (u[0]*v[1]-u[1]*v[0]).abs()>1e-10 || u[0]*v[0]+u[1]*v[1]<0. {break;}corners.pop();
        }
        corners.push(p);
    }
    let mut points=vec![start];let mut at=0;let mut shortcut_budget=1_000_000usize;
    while at+1<corners.len() {
        let mut next=at+1;
        for far in (at+2..=(at+32).min(corners.len()-1)).rev() {
            if shortcut_budget==0 {break;}
            if grid.line(corners[at],corners[far],|i| {
                if shortcut_budget==0 {return false;}shortcut_budget-=1;
                grid.cells[i]<2 && (grid.seen[i] || !grid.seen[grid.index(cell(corners[at])).unwrap()])
            }) {next=far;break;}
        }
        points.push(corners[next]);at=next;
    }
    ensure!(points.len()<=4096, "Visual route exceeds path point budget");
    let mut unknown=false;
    for p in points.windows(2) {
        ensure!(grid.line(p[0],p[1],|i| {unknown|=!grid.seen[i];grid.cells[i]!=3}), "Route intersects mapped obstacles; no path saved");
    }
    Ok(Route{points,crosses_unobserved_floor:unknown})
}
