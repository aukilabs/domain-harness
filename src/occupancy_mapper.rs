//! Portable current-state voxel projection. Input adapter binds the existing scene Product.
use anyhow::{Result, ensure};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;
use auki_registry::{AxisDirection, FrameRegistryEntry};
use auki_voxel_map::VoxelSnapshot;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::network::{CanonicalScene, SCENE_SCHEMA};

pub const SCHEMA: &str = "spatial-harness.occupancy-scene/v1";
pub const PRODUCT: &str = "occupancy-2d";
pub const UNKNOWN: i8 = -1;
pub const FREE: i8 = 0;
pub const OCCUPIED: i8 = 100;
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_CELLS: usize = 8192;

#[derive(Clone, Debug)]
pub struct Config {
    /// Height in meters along the source frame's positive up axis. Never inferred.
    pub floor_m: f64,
    pub interval: Duration,
    pub stale_after: Duration,
}
impl Config {
    pub fn new(floor_m: f64) -> Self {
        Self {
            floor_m,
            interval: Duration::from_secs(1),
            stale_after: Duration::from_secs(5),
        }
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.floor_m.is_finite() && self.floor_m.abs() < 1_000_000.,
            "invalid floor height"
        );
        ensure!(
            self.interval >= Duration::from_millis(100) && self.interval <= Duration::from_secs(60),
            "interval must be 0.1–60 seconds"
        );
        ensure!(
            self.stale_after >= self.interval,
            "stale timeout shorter than interval"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub index: [i32; 2],
    /// Row-major: v * (cell_size_m / resolution_m) + u.
    pub values: Vec<i8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OccupancyGrid {
    pub schema: String,
    pub frame: FrameRegistryEntry,
    /// Source coordinate indices for u and v. Positive axes as declared in frame.
    pub horizontal_axes: [usize; 2],
    pub up_axis: usize,
    pub floor_m: f64,
    pub obstacle_height_m: f64,
    pub resolution_m: f64,
    pub cell_size_m: f64,
    pub origin_uv_m: [f64; 2],
    /// Sparse cells; absent cells are unknown, never free. Full replacement map.
    pub cells: Vec<Cell>,
}

/// A separate scene with only an occupancy layer. No scene/layer/cell revisions.
pub fn cell_width(grid: &OccupancyGrid)->Result<usize> {
    ensure!(grid.resolution_m.is_finite() && (0.01..=0.1).contains(&grid.resolution_m),"Invalid occupancy resolution");
    let width=grid.cell_size_m/grid.resolution_m;
    ensure!(width.is_finite() && (1. ..=256.).contains(&width) && (width-width.round()).abs()<1e-6,"Invalid occupancy tile dimensions");
    Ok(width.round() as usize)
}

/// A separate scene with only an occupancy layer. No scene/layer/cell revisions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OccupancyScene {
    pub schema: String,
    pub scene_id: String,
    pub authority_peer_id: String,
    pub source_product: ProductReference,
    pub status: String,
    pub detail: String,
    pub layers: Layers,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layers {
    pub occupancy_2d: Option<OccupancyGrid>,
}
impl ContractType for OccupancyScene {
    const DATATYPE: &'static str = SCHEMA;
}

/// Validate a remote typed grid before adapting it to Home's existing tile view.
pub fn render_scene(scene: &OccupancyScene, owner: &str) -> Result<serde_json::Value> {
    use serde_json::json;
    ensure!(
        scene.schema == SCHEMA && scene.authority_peer_id == owner,
        "Occupancy schema/authority mismatch"
    );
    ensure!(
        size(scene) <= MAX_BYTES,
        "Occupancy scene exceeds size limit"
    );
    let grid = scene
        .layers
        .occupancy_2d
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Waiting for occupancy grid"))?;
    ensure!(
        grid.schema == "spatial-harness.occupancy-2d/v1"
            && grid.resolution_m.is_finite() && (0.01..=0.1).contains(&grid.resolution_m)
            && grid.obstacle_height_m == 1.,
        "Unsupported occupancy grid geometry"
    );
    let width=cell_width(grid)?;
    ensure!(
        grid.floor_m.is_finite()
            && grid.origin_uv_m.iter().all(|v| v.is_finite())
            && grid.cells.len() <= MAX_CELLS,
        "Invalid occupancy grid bounds"
    );
    ensure!(
        crate::conventions::registry(&grid.frame.peer_id, &grid.frame.frame_id)? == grid.frame,
        "Invalid occupancy frame"
    );
    let axes = [grid.frame.axes.x, grid.frame.axes.y, grid.frame.axes.z];
    let up = axes
        .iter()
        .position(|a| *a == AxisDirection::Up)
        .ok_or_else(|| anyhow::anyhow!("Occupancy frame has no up axis"))?;
    let horizontal: Vec<_> = (0..3).filter(|i| *i != up).collect();
    ensure!(
        grid.up_axis == up && grid.horizontal_axes == [horizontal[0], horizontal[1]],
        "Invalid occupancy axes"
    );
    let local = crate::local_mapper::is_local_frame(&grid.frame.frame_id);
    let display = if local {
        crate::local_mapper::display_frame(&grid.frame.frame_id)?
    } else {
        crate::model::FRAME.into()
    };
    let target = crate::conventions::registry(owner, &display)?;
    let mut tiles = Vec::new();
    let mut indices = std::collections::BTreeSet::new();
    let mut counts = [0usize; 3];
    for cell in &grid.cells {
        ensure!(
            indices.insert(cell.index) && cell.values.len() == width*width,
            "Invalid occupancy cell"
        );
        for (i, value) in cell.values.iter().enumerate() {
            let category = match *value {
                UNKNOWN => 0,
                FREE => 1,
                OCCUPIED => 2,
                _ => anyhow::bail!("Invalid occupancy value"),
            };
            counts[category] += 1;
            let mut center = [0.; 3];
            center[up] = grid.floor_m;
            center[horizontal[0]] = grid.origin_uv_m[0]
                + (f64::from(cell.index[0]) * width as f64 + (i % width) as f64 + 0.5) * grid.resolution_m;
            center[horizontal[1]] = grid.origin_uv_m[1]
                + (f64::from(cell.index[1]) * width as f64 + (i / width) as f64 + 0.5) * grid.resolution_m;
            let center = crate::conventions::point(center, &grid.frame, &target)?;
            tiles.push(json!({"center":center,"value":value}));
        }
    }
    Ok(
        json!({"frame":display,"local_map":local,"convention":"OpenGL · Y up · meters",
        "layer_type":"occupancy_2d","voxel_size":grid.resolution_m,"voxels":[],"occupancy_tiles":tiles,
        "occupancy_status":scene.status,"occupancy_counts":{"unknown":counts[0],"free":counts[1],"occupied":counts[2]},
        "authority_peer_id":scene.authority_peer_id,"source_product":scene.source_product,
        "local_mapping":{"detail":scene.detail},"portals":[],"objects":[],"notes":[],"sources":{},"demo":false}),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadCells {
    pub cells: Vec<[i32; 2]>,
}
impl ContractType for ReadCells {
    const DATATYPE: &'static str = "spatial-harness.occupancy-read-cells/v1";
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CellSelection {
    /// Always false: this response must not replace unrequested cells.
    pub complete_scene: bool,
    pub requested_cells: Vec<[i32; 2]>,
    pub scene: OccupancyScene,
}
impl ContractType for CellSelection {
    const DATATYPE: &'static str = "spatial-harness.occupancy-cell-selection/v1";
}

pub fn select_cells(scene: &OccupancyScene, request: ReadCells) -> Result<CellSelection> {
    ensure!(
        !request.cells.is_empty() && request.cells.len() <= 64,
        "request must contain 1–64 cells"
    );
    let keys: std::collections::BTreeSet<_> = request.cells.into_iter().collect();
    let mut result = scene.clone();
    if let Some(grid) = &mut result.layers.occupancy_2d {
        let width=cell_width(grid)?;
        let cells: BTreeMap<_, _> = grid.cells.drain(..).map(|c| (c.index, c)).collect();
        grid.cells = keys
            .iter()
            .map(|index| {
                cells.get(index).cloned().unwrap_or_else(|| Cell {
                    index: *index,
                    values: vec![UNKNOWN; width*width],
                })
            })
            .collect();
    }
    Ok(CellSelection {
        complete_scene: false,
        requested_cells: keys.into_iter().collect(),
        scene: result,
    })
}

/// Center rule: 0 < center height above floor <= 1m. All source samples must be
/// explicitly negative to call a column free. Zero evidence remains unknown.
pub fn project(voxels: &VoxelSnapshot, floor_m: f64) -> Result<OccupancyGrid> {
    Config::new(floor_m).validate()?;
    let layers = crate::voxel_layers::split(voxels)?;
    let d = &voxels.definition;
    let resolution = d.grid().voxel_size_m.0;
    ensure!(resolution.is_finite() && (0.01..=0.1).contains(&resolution), "unsupported source voxel resolution");
    let axes = [d.frame.axes.x, d.frame.axes.y, d.frame.axes.z];
    let up = axes
        .iter()
        .position(|a| *a == AxisDirection::Up)
        .ok_or_else(|| anyhow::anyhow!("source needs a positive up axis"))?;
    let horizontal: Vec<_> = (0..3).filter(|i| *i != up).collect();
    let horizontal_axes = [horizontal[0], horizontal[1]];
    let bottom = ((floor_m - d.grid_origin_m[up]) / resolution - 0.5 + 1e-8).floor() as i64 + 1;
    let top = ((floor_m + 1. - d.grid_origin_m[up]) / resolution - 0.5 + 1e-8).floor() as i64;
    // Keep source resolution. Every height in the one-metre band must be
    // explicitly observed free; an occupied height blocks the column.
    let mut columns = BTreeMap::<[i32; 2], (bool, std::collections::BTreeSet<[i64;3]>)>::new();
    for layer in &layers {
        for sample in layer.cells.iter().flat_map(|cell| &cell.voxels) {
            let p = sample.index.map(i64::from);
            if p[up] < bottom || p[up] > top {
                continue;
            }
            let u=i32::try_from(p[horizontal[0]])?;
            let v=i32::try_from(p[horizontal[1]])?;
                let entry = columns.entry([u,v]).or_default();
                if layer.kind == crate::voxel_layers::Kind::Occupied { entry.0 = true; }
                else { entry.1.insert(p); }
        }
    }
    let mut cells = BTreeMap::<[i32; 2], Vec<i8>>::new();
    let width=(2./resolution).ceil() as i32;
    for ([u, v], (hit, free)) in columns {
        let required=(top-bottom+1) as usize;
        let value = if hit {
            OCCUPIED
        } else if free.len() == required {
            FREE
        } else {
            UNKNOWN
        };
        if value == UNKNOWN {
            continue;
        }
        let cell_index = [u.div_euclid(width), v.div_euclid(width)];
        ensure!(
            cells.contains_key(&cell_index) || cells.len() < MAX_CELLS,
            "occupancy map exceeds 8192 spatial cells"
        );
        let values = cells
            .entry(cell_index)
            .or_insert_with(|| vec![UNKNOWN; (width*width) as usize]);
        values[(v.rem_euclid(width) * width + u.rem_euclid(width)) as usize] = value;
    }
    Ok(OccupancyGrid {
        schema: "spatial-harness.occupancy-2d/v1".into(),
        frame: d.frame.clone(),
        horizontal_axes,
        up_axis: up,
        floor_m,
        obstacle_height_m: 1.,
        resolution_m: resolution,
        cell_size_m: f64::from(width)*resolution,
        origin_uv_m: [
            d.grid_origin_m[horizontal[0]],
            d.grid_origin_m[horizontal[1]],
        ],
        cells: cells
            .into_iter()
            .map(|(index, values)| Cell { index, values })
            .collect(),
    })
}

fn size(scene: &OccupancyScene) -> usize {
    serde_json::to_vec(scene).map_or(usize::MAX, |b| b.len())
}

/// Owns a genuine SDK Product input binding and latest-only mailbox. The same
/// binding accepts a locally retained Product or a remote subscription's mirror.
type SceneInbox = Arc<Mutex<Option<(Arc<CanonicalScene>, Instant)>>>;

pub struct OccupancyMapper {
    _component: Component,
    _binding: ConfiguredBufferInput<CanonicalScene>,
    _input: InputPort<Observation<CanonicalScene>>,
    inbox: SceneInbox,
    source: RetainedProduct<CanonicalScene>,
    output: ConfiguredObservable<OccupancyScene>,
    capture: BufferProductCapture<OccupancyScene>,
    config: Config,
    state: OccupancyScene,
    last: Option<OccupancyScene>,
    started: Instant,
    last_cycle: Option<Instant>,
    last_received: Option<Instant>,
    timestamp: u64,
    served: Arc<Mutex<Option<OccupancyScene>>>,
}
impl OccupancyMapper {
    pub fn mount(
        runtime: &ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        source: &RetainedProduct<CanonicalScene>,
        owner: &str,
        clock: ClockReference,
        config: Config,
    ) -> Result<Self> {
        Self::mount_inner(runtime, Some(endpoint), source, owner, clock, config, None)
    }

    /// Home runs one mapper per independent local scene; IDs must not collide.
    pub fn mount_scoped(
        runtime: &ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        source: &RetainedProduct<CanonicalScene>,
        owner: &str,
        clock: ClockReference,
        config: Config,
        scene_id: &str,
    ) -> Result<Self> {
        ensure!(!scene_id.is_empty() && scene_id.len() <= 128, "invalid occupancy scene ID");
        Self::mount_inner(runtime, Some(endpoint), source, owner, clock, config, Some(scene_id))
    }

    /// Bind an SDK Product locally without mounting a network peer or endpoint.
    pub fn mount_local(
        runtime: &ComponentRuntime,
        source: &RetainedProduct<CanonicalScene>,
        owner: &str,
        clock: ClockReference,
        config: Config,
    ) -> Result<Self> {
        Self::mount_inner(runtime, None, source, owner, clock, config, None)
    }

    fn mount_inner(
        runtime: &ComponentRuntime,
        endpoint: Option<&ComponentProtocolEndpoint>,
        source: &RetainedProduct<CanonicalScene>,
        owner: &str,
        clock: ClockReference,
        config: Config,
        scene_id: Option<&str>,
    ) -> Result<Self> {
        config.validate()?;
        let component_id = scene_id.map_or_else(|| "voxel-to-occupancy-2d".into(), |id| format!("voxel-to-occupancy-2d:{id}"));
        let product_id = scene_id.map_or_else(|| PRODUCT.into(), |id| format!("{PRODUCT}:{id}"));
        let component = runtime.component(
            ComponentSpec::new(component_id)
                .product_input(ProductInputContract {
                    name: "source_scene".into(),
                    form: ProductForm::Buffer,
                    datatype: SCENE_SCHEMA.into(),
                    schema: SCENE_SCHEMA.into(),
                    exposure: Exposure::Cluster,
                })
                .observable(ObservableContract {
                    name: "scene".into(),
                    datatype: SCHEMA.into(),
                    schema: SCHEMA.into(),
                    access: vec![ObservationAccess::FollowNew],
                    exposure: Exposure::Cluster,
                })
                .operable(OperableContract {
                    name: "read_cells".into(),
                    instruction: ReadCells::DATATYPE.into(),
                    result: CellSelection::DATATYPE.into(),
                    exposure: Exposure::Cluster,
                }),
        )?;
        let inbox = Arc::new(Mutex::new(None));
        let mailbox = inbox.clone();
        let input =
            InputPort::<Observation<CanonicalScene>>::new("occupancy.source", move |entry| {
                *mailbox.lock().unwrap() = Some((entry.payload.payload.clone(), Instant::now()));
            });
        let binding = component.configured_buffer_input(
            "source_scene",
            source,
            // Project the retained current scene immediately, then follow new
            // observations. Latest starts after the high-water mark in the SDK.
            CursorStart::Current,
            &input,
        )?;
        let output = component.configured_observable(ConfiguredObservableSpec::new(
            "scene",
            format!("occupancy-{}", uuid::Uuid::new_v4()),
            clock,
            PayloadContract::Structured(StructuredPayloadContract {
                modality: "scenegraph".into(),
                datatype: SCHEMA.into(),
                schema: SCHEMA.into(),
                observes: "occupancy_2d_scene".into(),
                unit: Some("meters".into()),
            }),
        ))?;
        let served = Arc::new(Mutex::new(None::<OccupancyScene>));
        let state = served.clone();
        let read = component.operable(
            "read_cells",
            |context| !context.caller_peer_id.is_empty(),
            move |_, request: ReadCells| {
                let state = state
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?;
                let scene = state.as_ref().ok_or(InvocationError::TargetUnavailable)?;
                select_cells(scene, request).map_err(|e| InvocationError::Rejected(e.to_string()))
            },
        )?;
        component.expose()?;
        let capture = runtime.capture_buffer(
            product_id,
            &output,
            BufferLimits {
                max_entries: Some(2),
                max_bytes: Some(2 * MAX_BYTES),
                target_duration: None,
            },
            size,
        )?;
        if let Some(endpoint) = endpoint {
            endpoint.export_product(&capture.product())?;
            endpoint.export_operable(&read)?;
        }
        Ok(Self {
            _component: component,
            _binding: binding,
            _input: input,
            inbox,
            source: source.clone(),
            output,
            capture,
            config,
            state: OccupancyScene {
                schema: SCHEMA.into(),
                scene_id: scene_id.map_or_else(|| format!("{owner}:occupancy-2d"), str::to_owned),
                authority_peer_id: owner.into(),
                source_product: source.reference(),
                status: "waiting".into(),
                detail: "Waiting for source scene".into(),
                layers: Layers { occupancy_2d: None },
            },
            last: None,
            started: Instant::now(),
            last_cycle: None,
            last_received: None,
            timestamp: 0,
            served,
        })
    }
    pub fn product(&self) -> RetainedProduct<OccupancyScene> {
        self.capture.product()
    }
    pub fn interval(&self) -> Duration {
        self.config.interval
    }
    pub fn state(&self) -> &OccupancyScene {
        &self.state
    }
    /// Explicit operator clear, unlike unknown cells in ordinary observations.
    pub fn clear_geometry(&mut self) {
        self.state.layers.occupancy_2d = None;
        if let Ok(mut inbox)=self.inbox.lock(){inbox.take();}
        self.state.status="waiting".into();
        self.state.detail="Geometry cleared; waiting for new observations".into();
        self.last_received=None;
        self.last_cycle = None;
    }

    /// Call regularly; enforces rate limit and coalesces input. No history replay.
    pub fn tick(&mut self) -> Result<bool> {
        let now = Instant::now();
        if self
            .last_cycle
            .is_some_and(|t| now.duration_since(t) < self.config.interval)
        {
            return Ok(false);
        }
        // Anchor to the cadence rather than timer jitter, skipping missed slots.
        self.last_cycle = Some(self.last_cycle.map_or(now, |last| {
            let remainder = now.duration_since(last).as_nanos() % self.config.interval.as_nanos();
            now - Duration::from_nanos(remainder as u64)
        }));
        let next = self
            .inbox
            .lock()
            .map_err(|_| anyhow::anyhow!("occupancy inbox poisoned"))?
            .take();
        if let Some((scene, received)) = next {
            let result = (|| {
                scene.validate(&self.source.manifest.peer_id)?;
                ensure!(
                    crate::model::clock_ref(&scene.publication_clock) == self.source.producer.clock,
                    "source clock mismatch"
                );
                let observation = project(&scene.voxels, self.config.floor_m)?;
                accumulate(self.state.layers.occupancy_2d.as_ref(), observation)
            })();
            match result {
                Ok(grid) => {
                    self.last_received = Some(received);
                    self.state.layers.occupancy_2d = Some(grid);
                    self.state.status = "ready".into();
                    self.state.detail =
                        "Accumulated occupancy; only observed occupied/free samples replace prior evidence"
                            .into();
                }
                Err(e) => {
                    self.state.status = "invalid_input".into();
                    self.state.detail = e.to_string();
                }
            }
        }
        if self.source.end_notice().is_some() {
            self.state.status = "unavailable".into();
            self.state.detail = "Source Product closed; cached map only".into();
        } else if self.state.status == "ready"
            && self
                .last_received
                .is_some_and(|t| now.duration_since(t) > self.config.stale_after)
        {
            self.state.status = "stale".into();
            self.state.detail = "No recent source observation; cached map only".into();
        }
        if now.elapsed() > self.config.interval {
            eprintln!(
                "occupancy mapper cycle overrun: {:?} (target {:?})",
                now.elapsed(),
                self.config.interval
            );
        }
        if self.last.as_ref() == Some(&self.state) {
            return Ok(false);
        }
        ensure!(
            size(&self.state) <= MAX_BYTES,
            "occupancy scene exceeds size limit"
        );
        self.timestamp = (self.started.elapsed().as_nanos() as u64).max(self.timestamp + 1);
        self.output
            .publish(self.timestamp, Arc::new(self.state.clone()))?;
        ensure!(
            self.capture.errors().is_empty(),
            "occupancy publication retention failed"
        );
        self.last = Some(self.state.clone());
        *self
            .served
            .lock()
            .map_err(|_| anyhow::anyhow!("occupancy state poisoned"))? = Some(self.state.clone());
        Ok(true)
    }
}

/// Unknown means no new evidence, not a request to erase the accumulated map.
/// Build a candidate first so incompatible geometry/capacity cannot corrupt it.
fn accumulate(
    previous: Option<&OccupancyGrid>,
    mut observation: OccupancyGrid,
) -> Result<OccupancyGrid> {
    let Some(previous) = previous else {
        return Ok(observation);
    };
    let mut old_geometry = previous.clone();
    old_geometry.cells.clear();
    let incoming = std::mem::take(&mut observation.cells);
    ensure!(
        old_geometry == observation,
        "occupancy geometry changed; restart mapper for the new frame/grid"
    );
    let mut cells: BTreeMap<_, _> = previous
        .cells
        .iter()
        .cloned()
        .map(|c| (c.index, c))
        .collect();
    for cell in incoming {
        if let Some(existing) = cells.get_mut(&cell.index) {
            for (old, new) in existing.values.iter_mut().zip(cell.values) {
                if new != UNKNOWN {
                    *old = new;
                }
            }
        } else {
            cells.insert(cell.index, cell);
        }
        ensure!(
            cells.len() <= MAX_CELLS,
            "accumulated occupancy exceeds 8192 spatial cells"
        );
    }
    observation.cells = cells.into_values().collect();
    Ok(observation)
}
