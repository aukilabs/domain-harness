//! Independently readable views of one reconciled occupancy accumulator.
//! Missing and zero-evidence voxels are unknown, never observed free.
use anyhow::{Result, ensure};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;
use auki_datatypes::map::MapUpdate;
use auki_registry::FrameRegistryEntry;
use auki_voxel_map::VoxelSnapshot;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub const SCHEMA: &str = "spatial-harness.voxel-layer/v1";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Occupied,
    ObservedFree,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    pub index: [i32; 3],
    pub voxel: crate::raycast::Voxel,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cell {
    pub index: [i32; 3],
    pub voxels: Vec<Sample>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub schema: String,
    pub kind: Kind,
    pub frame: FrameRegistryEntry,
    pub grid_origin_m: [f64; 3],
    pub resolution_m: f64,
    pub cell_size_m: f64,
    /// None is a full layer; Some is a spatial selection. An absent requested
    /// cell contains no evidence in this layer, not evidence of free space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_cells: Option<Vec<[i32; 3]>>,
    pub cells: Vec<Cell>,
}
impl ContractType for Layer {
    const DATATYPE: &'static str = SCHEMA;
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ReadCells {
    pub cells: Vec<[i32; 3]>,
}
impl ContractType for ReadCells {
    const DATATYPE: &'static str = "spatial-harness.voxel-read-cells/v1";
}
impl Layer {
    pub fn select(&self, request: &ReadCells) -> Result<Self> {
        ensure!(
            request.cells.len() <= 4096,
            "at most 4096 cells per request"
        );
        let wanted: std::collections::BTreeSet<_> = request.cells.iter().collect();
        Ok(Self {
            cells: self
                .cells
                .iter()
                .filter(|c| wanted.contains(&c.index))
                .cloned()
                .collect(),
            schema: self.schema.clone(),
            kind: self.kind.clone(),
            frame: self.frame.clone(),
            grid_origin_m: self.grid_origin_m,
            resolution_m: self.resolution_m,
            cell_size_m: self.cell_size_m,
            requested_cells: Some(wanted.into_iter().copied().collect()),
        })
    }
}
pub fn split(snapshot: &VoxelSnapshot) -> Result<[Layer; 2]> {
    let d = &snapshot.definition;
    ensure!(
        (d.grid().voxel_size_m.0 - 0.1).abs() < 1e-9,
        "layers require 10 cm voxels"
    );
    snapshot.accumulator()?; // Validate coordinates, finite evidence and provenance.
    // The SDK's surface viewer intentionally omits free evidence at every
    // threshold. Read the validated checkpoint rather than that render view.
    let checkpoint = MapUpdate::decode(snapshot.checkpoint.as_slice())?
        .checkpoint
        .ok_or_else(|| anyhow::anyhow!("voxel layers require a full checkpoint"))?;
    let mut cells: [BTreeMap<[i32; 3], Vec<Sample>>; 2] = Default::default();
    for chunk in checkpoint.voxel_chunks {
        for v in chunk.voxels {
            if v.occupancy_evidence == 0. {
                continue;
            }
            let base = [chunk.chunk_x, chunk.chunk_y, chunk.chunk_z];
            let local = [v.x, v.y, v.z];
            let mut index = [0; 3];
            for i in 0..3 {
                index[i] = i32::try_from(
                    i64::from(base[i]) * i64::from(d.grid().chunk_dimension) + i64::from(local[i]),
                )?;
            }
            let layer = usize::from(v.occupancy_evidence < 0.);
            cells[layer]
                .entry(index.map(|x| x.div_euclid(20)))
                .or_default()
                .push(Sample {
                    index,
                    voxel: crate::raycast::Voxel {
                        center: std::array::from_fn(|i| {
                            (f64::from(index[i]) + 0.5) * 0.1 + d.grid_origin_m[i]
                        }),
                        evidence: v.occupancy_evidence,
                        linear_rgb: if layer == 0 {
                            v.color.filter(|c| c.weight > 0.).map(|c| {
                                [c.red_sum, c.green_sum, c.blue_sum]
                                    .map(|s| (s / c.weight).clamp(0., 1.))
                            })
                        } else {
                            None
                        },
                    },
                });
        }
    }
    let mut layers = cells.map(|cells| Layer {
        schema: SCHEMA.into(),
        kind: Kind::Occupied,
        frame: d.frame.clone(),
        grid_origin_m: d.grid_origin_m,
        resolution_m: 0.1,
        cell_size_m: 2.,
        requested_cells: None,
        cells: cells
            .into_iter()
            .map(|(index, voxels)| Cell { index, voxels })
            .collect(),
    });
    layers[1].kind = Kind::ObservedFree;
    Ok(layers)
}

pub struct Publisher {
    _component: Component,
    output: ConfiguredObservable<Layer>,
    capture: BufferProductCapture<Layer>,
    served: Arc<Mutex<Option<Layer>>>,
}
impl Publisher {
    pub fn mount(
        runtime: &ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        id: &str,
        name: &str,
        clock: ClockReference,
        frame: FrameRegistryEntry,
    ) -> Result<Self> {
        let component = runtime.component(
            ComponentSpec::new(format!("{name}:{id}"))
                .observable(ObservableContract {
                    name: "layer".into(),
                    datatype: SCHEMA.into(),
                    schema: SCHEMA.into(),
                    access: vec![ObservationAccess::FollowNew],
                    exposure: Exposure::Cluster,
                })
                .operable(OperableContract {
                    name: "read_cells".into(),
                    instruction: ReadCells::DATATYPE.into(),
                    result: SCHEMA.into(),
                    exposure: Exposure::Cluster,
                }),
        )?;
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "layer",
                uuid::Uuid::new_v4().to_string(),
                clock,
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "voxels".into(),
                    datatype: SCHEMA.into(),
                    schema: SCHEMA.into(),
                    observes: name.into(),
                    unit: Some("meters".into()),
                }),
            )
            .in_registered_frame(frame),
        )?;
        let served = Arc::new(Mutex::new(None::<Layer>));
        let state = served.clone();
        let read = component.operable(
            "read_cells",
            |ctx| !ctx.caller_peer_id.is_empty(),
            move |_, request: ReadCells| {
                let state = state
                    .lock()
                    .map_err(|_| InvocationError::TargetUnavailable)?;
                state
                    .as_ref()
                    .ok_or(InvocationError::TargetUnavailable)?
                    .select(&request)
                    .map_err(|e| InvocationError::Rejected(e.to_string()))
            },
        )?;
        component.expose()?;
        endpoint.export_operable(&read)?;
        let capture = runtime.capture_buffer(
            format!("{name}:{id}"),
            &output,
            BufferLimits {
                max_entries: Some(1),
                max_bytes: Some(64 * 1024 * 1024),
                target_duration: None,
            },
            |s| serde_json::to_vec(s).map_or(usize::MAX, |v| v.len()),
        )?;
        endpoint.export_product(&capture.product())?;
        Ok(Self {
            _component: component,
            output,
            capture,
            served,
        })
    }
    pub fn publish(&self, timestamp: u64, layer: Layer) -> Result<()> {
        *self
            .served
            .lock()
            .map_err(|_| anyhow::anyhow!("voxel layer unavailable"))? = Some(layer.clone());
        self.output.publish(timestamp, Arc::new(layer))?;
        ensure!(
            self.capture.errors().is_empty(),
            "voxel layer retention failed"
        );
        Ok(())
    }
}
