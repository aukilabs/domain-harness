use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Room {
    pub description: String,
    pub floor_rectangles: Vec<[f64; 4]>,
    pub boxes: Vec<RoomBox>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomBox {
    pub name: String,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub color: String,
}
impl Room {
    pub fn load() -> Result<Self> {
        let room: Self = serde_json::from_slice(&std::fs::read("config/synthetic-room.json")?)?;
        ensure!(
            !room.floor_rectangles.is_empty() && room.boxes.len() < 64,
            "invalid room layout"
        );
        for r in &room.floor_rectangles {
            ensure!(
                r.iter().all(|x| x.is_finite()) && r[0] < r[1] && r[2] < r[3],
                "invalid floor rectangle"
            );
        }
        for b in &room.boxes {
            ensure!(
                (0..3).all(|i| b.min[i].is_finite() && b.max[i].is_finite() && b.min[i] < b.max[i]),
                "invalid room box"
            );
        }
        Ok(room)
    }
    pub fn contains_floor(&self, x: f64, z: f64) -> bool {
        self.floor_rectangles
            .iter()
            .any(|r| x >= r[0] && x < r[1] && z >= r[2] && z < r[3])
    }
    /// Analytic first-visible surface along a synthetic depth ray. The result is
    /// moved a micron into the solid to avoid voxel-boundary rounding ambiguity.
    pub fn hit(&self, origin: [f64; 3], target: [f64; 3]) -> [f64; 3] {
        let direction: [f64; 3] = std::array::from_fn(|i| target[i] - origin[i]);
        let mut first: f64 = 1.;
        for b in &self.boxes {
            let mut near: f64 = 0.;
            let mut far: f64 = 1.;
            for i in 0..3 {
                // Simulate the same 10 cm occupancy geometry consumed by the
                // mapper: grazing rays must not clear a partly filled solid cell.
                let low = (b.min[i] * 10. + 1e-8).floor() / 10.;
                let high = (b.max[i] * 10. - 1e-8).ceil() / 10.;
                if direction[i].abs() < 1e-12 {
                    if origin[i] < low || origin[i] > high {
                        far = -1.;
                        break;
                    }
                } else {
                    let a = (low - origin[i]) / direction[i];
                    let c = (high - origin[i]) / direction[i];
                    near = near.max(a.min(c));
                    far = far.min(a.max(c));
                }
            }
            if near <= far {
                first = first.min(near + 1e-6);
            }
        }
        std::array::from_fn(|i| origin[i] + direction[i] * first)
    }
}
