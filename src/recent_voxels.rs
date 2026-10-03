//! Single-writer live occupancy, using the latest three observations of each cell.
//! The SDK raycaster supplies visibility; the SDK accumulator consumes an explicit
//! replacement checkpoint. Never clamp or reinterpret SDK additive evidence.
use anyhow::{Result, ensure};
use auki_datatypes::map::{
    ColorEvidence, MapUpdate, VoxelChunkSnapshot, VoxelMapCheckpoint, VoxelSnapshot,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

type Key = ([i32; 3], [u32; 3]);
#[derive(Clone)]
struct Vote {
    hit: bool,
    color: Option<[f64; 3]>,
}
#[derive(Clone, Default)]
struct CellHistory {
    votes: VecDeque<Vote>,
    last_seen: u64,
}
#[derive(Clone, Default)]
pub struct RecentVoxels {
    cells: BTreeMap<Key, CellHistory>,
    generation: u64,
}
impl RecentVoxels {
    pub fn observe(&mut self, rays: &MapUpdate, endpoints: &MapUpdate) -> Result<MapUpdate> {
        ensure!(
            rays.checkpoint.is_none() && endpoints.checkpoint.is_none(),
            "expected sensor deltas"
        );
        // Order retention by this writer's observations, never by clocks from
        // different robots. Wrapping is harmless for the practically unreachable
        // rollover: it changes eviction priority, not occupancy evidence.
        self.generation = self.generation.wrapping_add(1);
        let mut seen = BTreeSet::new();
        let mut hits = BTreeSet::new();
        let mut colors = BTreeMap::<Key, [f64; 4]>::new();
        for chunk in &endpoints.voxel_chunks {
            for v in &chunk.voxels {
                if v.occupancy_delta > 0. {
                    hits.insert((
                        [chunk.chunk_x, chunk.chunk_y, chunk.chunk_z],
                        [v.x, v.y, v.z],
                    ));
                }
            }
        }
        for chunk in &rays.voxel_chunks {
            for v in &chunk.voxels {
                let key = (
                    [chunk.chunk_x, chunk.chunk_y, chunk.chunk_z],
                    [v.x, v.y, v.z],
                );
                // A net-zero ray delta can be a hit crossed by another ray.
                if v.occupancy_delta != 0. || hits.contains(&key) {
                    seen.insert(key);
                }
                if let Some(c) = &v.color {
                    let sums = colors.entry(key).or_default();
                    for (sum, delta) in sums.iter_mut().zip([
                        c.red_sum_delta,
                        c.green_sum_delta,
                        c.blue_sum_delta,
                        c.weight_delta,
                    ]) {
                        *sum += f64::from(delta);
                    }
                }
            }
        }
        seen.extend(hits.iter().copied());
        for key in seen {
            let hit = hits.contains(&key);
            let color = colors
                .get(&key)
                .filter(|c| hit && c[3] > 0.)
                .map(|c| [c[0] / c[3], c[1] / c[3], c[2] / c[3]]);
            let history = self.cells.entry(key).or_default();
            if history.votes.len() == 3 {
                history.votes.pop_front();
            }
            history.votes.push_back(Vote { hit, color });
            history.last_seen = self.generation;
        }
        if self.cells.len() > auki_voxel_map::MAX_VOXELS {
            // Free-space rays visit far more cells than visible surfaces. Keep
            // every history containing a hit, including mixed histories still
            // participating in the three-observation clearing policy. Retiring
            // an all-free cell makes it unknown; it never invents occupied space.
            let mut free: Vec<_> = self
                .cells
                .iter()
                .filter(|(_, h)| h.votes.iter().all(|v| !v.hit))
                .map(|(key, h)| (h.last_seen, *key))
                .collect();
            free.sort_unstable();
            ensure!(
                self.cells.len() - free.len() <= auki_voxel_map::MAX_VOXELS,
                "occupied voxel history exceeds map bound"
            );
            // Leave headroom so small subsequent scans do not sort the entire
            // history on every observation. Recently observed free cells survive.
            let target = auki_voxel_map::MAX_VOXELS * 7 / 8;
            for (_, key) in free
                .into_iter()
                .take(self.cells.len().saturating_sub(target))
            {
                self.cells.remove(&key);
            }
        }
        ensure!(
            self.cells.len() <= auki_voxel_map::MAX_VOXELS,
            "live voxel history exceeds map bound"
        );
        let mut chunks = BTreeMap::<[i32; 3], Vec<VoxelSnapshot>>::new();
        for ((chunk, cell), history) in &self.cells {
            let occupancy = history
                .votes
                .iter()
                .map(|v| if v.hit { 1. } else { -1. })
                .sum();
            let mut color = None::<ColorEvidence>;
            if occupancy > 0. {
                for rgb in history.votes.iter().filter_map(|v| v.color) {
                    let c = color.get_or_insert_with(Default::default);
                    c.red_sum += rgb[0];
                    c.green_sum += rgb[1];
                    c.blue_sum += rgb[2];
                    c.weight += 1.;
                }
            }
            chunks.entry(*chunk).or_default().push(VoxelSnapshot {
                x: cell[0],
                y: cell[1],
                z: cell[2],
                occupancy_evidence: occupancy,
                semantics: vec![],
                color,
            });
        }
        Ok(MapUpdate {
            voxel_chunks: vec![],
            checkpoint: Some(VoxelMapCheckpoint {
                voxel_chunks: chunks
                    .into_iter()
                    .map(|(c, voxels)| VoxelChunkSnapshot {
                        chunk_x: c[0],
                        chunk_y: c[1],
                        chunk_z: c[2],
                        voxels,
                    })
                    .collect(),
            }),
        })
    }
}
