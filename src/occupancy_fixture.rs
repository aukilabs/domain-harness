//! Explicit synthetic source for robot-free demos and opt-in Home simulation.
use crate::{
    model,
    network::{CanonicalScene, SCENE_SCHEMA},
};
use anyhow::Result;
use auki_components::*;
use auki_datatypes::map::{MapUpdate, VoxelChunkSnapshot, VoxelMapCheckpoint, VoxelSnapshot};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    #[default]
    Obstacle,
    Clear,
    Overhead,
    Unknown,
}

pub struct FakeVoxelSource {
    _component: Component,
    output: ConfiguredObservable<CanonicalScene>,
    capture: BufferProductCapture<CanonicalScene>,
    template: CanonicalScene,
    session_clock: auki_time::SessionClock,
    surveyed: BTreeSet<[i32; 2]>,
}
impl FakeVoxelSource {
    pub fn new(
        runtime: &ComponentRuntime,
        owner: &str,
        session_clock: auki_time::SessionClock,
    ) -> Result<Self> {
        let clock = session_clock.registry_entry();
        anyhow::ensure!(clock.peer_id == owner, "Synthetic source clock owner mismatch");
        // Reuse the explicit synthetic fixture's valid source/calibration contracts.
        let fixture = model::fixtures()?;
        let event = model::Event::Contribution(serde_json::from_value(
            fixture["contributions"][0].clone(),
        )?);
        let built = model::build(&[event])?;
        let component = runtime.component(ComponentSpec::new("fake-voxel-source").observable(
            ObservableContract {
                name: "scene".into(),
                datatype: SCENE_SCHEMA.into(),
                schema: SCENE_SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            },
        ))?;
        let output = component.configured_observable(ConfiguredObservableSpec::new(
            "scene",
            format!("fake-{}", uuid::Uuid::new_v4()),
            model::clock_ref(&clock),
            PayloadContract::Structured(StructuredPayloadContract {
                modality: "scenegraph".into(),
                datatype: SCENE_SCHEMA.into(),
                schema: SCENE_SCHEMA.into(),
                observes: "synthetic_voxel_test_room".into(),
                unit: Some("meters".into()),
            }),
        ))?;
        component.expose()?;
        let capture = runtime.capture_buffer(
            "fake-voxels-scene",
            &output,
            BufferLimits::entries(2),
            |s| serde_json::to_vec(s).map_or(usize::MAX, |v| v.len()),
        )?;
        let template = CanonicalScene {
            schema: SCENE_SCHEMA.into(),
            sdk_revision: model::SDK_REV.into(),
            authority_peer_id: owner.into(),
            frame: built.portals.scenegraph.map.frame.id.clone(),
            publication_clock: clock,
            portals: built.portals,
            voxels: built.voxels,
            objects: vec![],
            registries: serde_json::json!({"simulation":true}),
        };
        Ok(Self {
            _component: component,
            output,
            capture,
            template,
            session_clock,
            surveyed: BTreeSet::new(),
        })
    }
    pub fn product(&self) -> RetainedProduct<CanonicalScene> {
        self.capture.product()
    }
    pub fn publish(&mut self, scenario: Scenario) -> Result<Arc<CanonicalScene>> {
        self.publish_inner(scenario, None)
    }
    /// Deterministic 60-second patrol. Publish only the current 1.5m footprint.
    /// This is synthetic evidence, not a sensor raycaster or navigation stack.
    pub fn publish_patrol(&mut self, seconds: f64) -> Result<Arc<CanonicalScene>> {
        self.publish_patrol_phase(seconds, 0.)
    }
    pub fn publish_patrol_phase(
        &mut self,
        seconds: f64,
        phase: f64,
    ) -> Result<Arc<CanonicalScene>> {
        anyhow::ensure!(
            seconds.is_finite() && seconds >= 0.,
            "invalid simulation time"
        );
        anyhow::ensure!(phase.is_finite(), "invalid patrol phase");
        let angle = seconds * std::f64::consts::TAU / 60. + phase;
        let position = [1.8 * angle.cos(), 0., 1.8 * angle.sin()];
        self.surveyed.clear();
        for x in -30..30 {
            for z in -30..30 {
                if ((x as f64 + 0.5) * 0.1 - position[0])
                    .hypot((z as f64 + 0.5) * 0.1 - position[2])
                    <= 1.5
                {
                    self.surveyed.insert([x, z]);
                }
            }
        }
        // ROS robot-body (+X forward, +Z up) to the map's OpenGL Y-up frame.
        let yaw = -angle - std::f64::consts::FRAC_PI_2;
        let (s, c) = (yaw / 2.).sin_cos();
        let h = std::f64::consts::FRAC_1_SQRT_2;
        self.publish_inner(
            Scenario::Obstacle,
            Some(serde_json::json!({
                "frame": self.template.frame,
                "pose": {"translation":position,"rotation_xyzw":[-c*h,s*h,s*h,c*h]},
                "simulation_seconds": seconds
            })),
        )
    }
    fn publish_inner(
        &mut self,
        scenario: Scenario,
        robot: Option<serde_json::Value>,
    ) -> Result<Arc<CanonicalScene>> {
        let mut scene = self.template.clone();
        // A 60cm-wide, 80cm-high obstacle shuttles every 12 seconds.
        // Rebuild evidence from current observations: its old location becomes
        // explicitly free only while observed, never merely because it vanished.
        let moving_z = robot
            .as_ref()
            .and_then(|r| r["simulation_seconds"].as_f64())
            .map(|seconds| 0.6 * (seconds * std::f64::consts::TAU / 12.).sin());
        let mut chunks = BTreeMap::<[i32; 3], Vec<VoxelSnapshot>>::new();
        let mut put = |x: i32, y: i32, z: i32, evidence: f64| {
            chunks
                .entry([x.div_euclid(16), y.div_euclid(16), z.div_euclid(16)])
                .or_default()
                .push(VoxelSnapshot {
                    x: x.rem_euclid(16) as u32,
                    y: y.rem_euclid(16) as u32,
                    z: z.rem_euclid(16) as u32,
                    occupancy_evidence: evidence,
                    semantics: vec![],
                    color: None,
                });
        };
        // A 6×6m surveyed area; the north-east corner remains unobserved.
        for x in -30..30 {
            for z in -30..30 {
                if robot.is_some() && !self.surveyed.contains(&[x, z]) {
                    continue;
                }
                put(x, -1, z, 1.); // Floor centers at -5cm: intentionally ignored.
                if (x >= 20 && z >= 20)
                    || (scenario == Scenario::Unknown
                        && (-5..5).contains(&x)
                        && (-5..5).contains(&z))
                {
                    continue;
                }
                for y in 0..10 {
                    let wall = x == -30 || x == 29 || z == -30 || z == 29;
                    let block = scenario == Scenario::Obstacle
                        && (-5..5).contains(&x)
                        && (-5..5).contains(&z)
                        && y < 8;
                    let moving = moving_z.is_some_and(|center_z| {
                        ((x as f64 + 0.5) * 0.1 - 1.2).abs() < 0.3
                            && ((z as f64 + 0.5) * 0.1 - center_z).abs() < 0.3
                            && y < 8
                    });
                    put(x, y, z, if wall || block || moving { 1. } else { -1. });
                }
                if scenario == Scenario::Overhead && (-5..5).contains(&x) && (-5..5).contains(&z) {
                    put(x, 15, z, 1.);
                }
            }
        }
        scene.voxels.checkpoint = MapUpdate {
            voxel_chunks: vec![],
            checkpoint: Some(VoxelMapCheckpoint {
                voxel_chunks: chunks
                    .into_iter()
                    .map(|(p, voxels)| VoxelChunkSnapshot {
                        chunk_x: p[0],
                        chunk_y: p[1],
                        chunk_z: p[2],
                        voxels,
                    })
                    .collect(),
            }),
        }
        .encode_to_vec();
        scene.registries["scenario"] = serde_json::to_value(scenario)?;
        if let Some(robot) = robot {
            scene.registries["local_mapping"] = serde_json::json!({"robot_pose":robot});
        }
        self.publish_scene(scene)
    }

    /// Merge a synchronized observation round in the simulator's shared frame.
    /// This does not estimate inter-robot alignment; both sources know that frame.
    pub fn publish_combined(
        &mut self,
        sources: &[Arc<CanonicalScene>],
    ) -> Result<Arc<CanonicalScene>> {
        anyhow::ensure!(
            !sources.is_empty() && sources.len() <= 2,
            "expected one or two robot sources"
        );
        let mut scene = self.template.clone();
        let mut chunks = BTreeMap::<[i32; 3], BTreeMap<[u32; 3], VoxelSnapshot>>::new();
        let mut runners = serde_json::Map::new();
        let mut contributors = Vec::new();
        let time = &sources[0].registries["local_mapping"]["robot_pose"]["simulation_seconds"];
        for (i, source) in sources.iter().enumerate() {
            source.validate(&source.authority_peer_id)?;
            anyhow::ensure!(
                source.frame == scene.frame
                    && source.voxels.definition.frame == scene.voxels.definition.frame
                    && source.voxels.definition.grid_origin_m
                        == scene.voxels.definition.grid_origin_m
                    && source.voxels.definition.grid().voxel_size_m
                        == scene.voxels.definition.grid().voxel_size_m
                    && source.voxels.definition.grid().chunk_dimension
                        == scene.voxels.definition.grid().chunk_dimension,
                "incompatible collaborative voxel frame/grid"
            );
            let robot = &source.registries["local_mapping"]["robot_pose"];
            anyhow::ensure!(
                !time.is_null() && &robot["simulation_seconds"] == time,
                "unsynchronized simulation round"
            );
            runners.insert(
                source.authority_peer_id.clone(),
                serde_json::json!({"name":format!("Robot {}",i+1),"robot_pose":robot}),
            );
            contributors.push(source.authority_peer_id.clone());
            let update = MapUpdate::decode(source.voxels.checkpoint.as_slice())?;
            let checkpoint = update
                .checkpoint
                .ok_or_else(|| anyhow::anyhow!("missing checkpoint"))?;
            for chunk in checkpoint.voxel_chunks {
                let dest = chunks
                    .entry([chunk.chunk_x, chunk.chunk_y, chunk.chunk_z])
                    .or_default();
                for voxel in chunk.voxels {
                    let key = [voxel.x, voxel.y, voxel.z];
                    // Simultaneous conflicts are conservative: occupied wins.
                    if dest
                        .get(&key)
                        .is_none_or(|old| voxel.occupancy_evidence > old.occupancy_evidence)
                    {
                        dest.insert(key, voxel);
                    }
                }
            }
        }
        scene.voxels.checkpoint = MapUpdate {
            voxel_chunks: vec![],
            checkpoint: Some(VoxelMapCheckpoint {
                voxel_chunks: chunks
                    .into_iter()
                    .map(|(p, v)| VoxelChunkSnapshot {
                        chunk_x: p[0],
                        chunk_y: p[1],
                        chunk_z: p[2],
                        voxels: v.into_values().collect(),
                    })
                    .collect(),
            }),
        }
        .encode_to_vec();
        scene.registries = serde_json::json!({"simulation":true,"contributors":contributors,"local_mapping":{"runners":runners,"robot_pose":sources[0].registries["local_mapping"]["robot_pose"]}});
        self.publish_scene(scene)
    }
    fn publish_scene(&mut self, scene: CanonicalScene) -> Result<Arc<CanonicalScene>> {
        scene.validate(&scene.authority_peer_id)?;
        let scene = Arc::new(scene);
        self.output.publish(self.session_clock.now_ns(), scene.clone())?;
        anyhow::ensure!(
            self.capture.errors().is_empty(),
            "fake source retention failed"
        );
        Ok(scene)
    }
}
