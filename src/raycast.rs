use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Voxel {
    pub center: [f64; 3],
    pub evidence: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linear_rgb: Option<[f64; 3]>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Hit {
    pub position: [f64; 3],
    pub center: [f64; 3],
    pub distance: f64,
}

/// Exact ray/AABB intersections against occupied SDK cells. Unknown and free
/// space never count as a surface. Linear scan is deliberate for this tiny demo.
pub fn raycast(
    voxels: &[Voxel],
    size: f64,
    origin: [f64; 3],
    direction: [f64; 3],
    max_distance: f64,
) -> Result<Option<Hit>> {
    ensure!(
        origin.iter().chain(direction.iter()).all(|x| x.is_finite()),
        "non-finite ray"
    );
    ensure!(
        size.is_finite()
            && size > 0.
            && max_distance.is_finite()
            && max_distance > 0.
            && max_distance <= 100.,
        "invalid ray bounds"
    );
    let norm = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
    ensure!(
        norm.is_finite() && norm > 1e-12,
        "zero or invalid ray direction"
    );
    let d = direction.map(|v| v / norm);
    let mut result: Option<Hit> = None;
    for voxel in voxels.iter().filter(|v| v.evidence > 0.) {
        let mut near: f64 = 0.;
        let mut far = max_distance;
        for axis in 0..3 {
            let low = voxel.center[axis] - size / 2.;
            let high = voxel.center[axis] + size / 2.;
            if d[axis].abs() < 1e-12 {
                if origin[axis] < low || origin[axis] > high {
                    far = -1.;
                    break;
                }
            } else {
                let a = (low - origin[axis]) / d[axis];
                let b = (high - origin[axis]) / d[axis];
                near = near.max(a.min(b));
                far = far.min(a.max(b));
            }
        }
        if near <= far && result.as_ref().is_none_or(|hit| near < hit.distance) {
            result = Some(Hit {
                position: std::array::from_fn(|i| origin[i] + d[i] * near),
                center: voxel.center,
                distance: near,
            });
        }
    }
    Ok(result)
}

/// Sweep a sphere along the ray. Radius is half the beam diameter. Return the
/// contact on the original voxel, not on its inflated bounds. Piecewise squared
/// distance to an AABB handles faces, rounded edges and corners exactly.
pub fn beamcast(
    voxels: &[Voxel],
    size: f64,
    origin: [f64; 3],
    direction: [f64; 3],
    max_distance: f64,
    radius: f64,
) -> Result<Option<Hit>> {
    raycast(&[], size, origin, direction, max_distance)?;
    ensure!(
        radius.is_finite() && radius > 0. && radius <= 1.,
        "Invalid beam radius"
    );
    let norm = direction.iter().map(|x| x * x).sum::<f64>().sqrt();
    let d = direction.map(|x| x / norm);
    let mut result: Option<Hit> = None;
    for v in voxels.iter().filter(|v| v.evidence > 0.) {
        let low = v.center.map(|x| x - size / 2.);
        let high = v.center.map(|x| x + size / 2.);
        let limit = result.as_ref().map_or(max_distance, |h| h.distance);
        let mut cuts = vec![0., limit];
        for a in 0..3 {
            if d[a].abs() > 1e-12 {
                for edge in [low[a], high[a]] {
                    let t = (edge - origin[a]) / d[a];
                    if t > 0. && t < limit {
                        cuts.push(t);
                    }
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        for interval in cuts.windows(2) {
            let (start, end) = (interval[0], interval[1]);
            let mid = (start + end) / 2.;
            let (mut qa, mut qb, mut qc) = (0., 0., -radius * radius);
            for a in 0..3 {
                let p = origin[a] + d[a] * mid;
                let edge = if p < low[a] {
                    low[a]
                } else if p > high[a] {
                    high[a]
                } else {
                    continue;
                };
                let offset = origin[a] - edge;
                qa += d[a] * d[a];
                qb += 2. * d[a] * offset;
                qc += offset * offset;
            }
            let t = if qa * start * start + qb * start + qc <= 1e-12 {
                start
            } else if qa > 1e-16 {
                let discriminant = qb * qb - 4. * qa * qc;
                if discriminant < -1e-12 {
                    continue;
                }
                (-qb - discriminant.max(0.).sqrt()) / (2. * qa)
            } else {
                continue;
            };
            if t < start - 1e-9 || t > end + 1e-9 {
                continue;
            }
            let t = t.clamp(start, end);
            if result.as_ref().is_none_or(|h| t < h.distance) {
                result = Some(Hit {
                    position: std::array::from_fn(|a| {
                        (origin[a] + d[a] * t).clamp(low[a], high[a])
                    }),
                    center: v.center,
                    distance: t,
                });
            }
            break;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ray_hits_nearest_surface_not_cell_center_or_free_space() {
        let v = vec![
            Voxel {
                linear_rgb: None,
                center: [0., 0., 4.],
                evidence: 2.,
            },
            Voxel {
                linear_rgb: None,
                center: [0., 0., 2.],
                evidence: 1.,
            },
            Voxel {
                linear_rgb: None,
                center: [0., 0., 1.],
                evidence: -1.,
            },
        ];
        let h = raycast(&v, 1., [0.; 3], [0., 0., 10.], 10.)
            .unwrap()
            .unwrap();
        assert_eq!(h.position, [0., 0., 1.5]);
        assert!(
            raycast(&v, 1., [2., 0., 0.], [0., 0., 1.], 10.)
                .unwrap()
                .is_none()
        );
        assert!(
            raycast(&v, 1., [0.; 3], [0., 0., 1.], 1.)
                .unwrap()
                .is_none()
        );
        assert!(raycast(&v, 1., [0.; 3], [0.; 3], 10.).is_err());
    }
    #[test]
    fn negative_cells_and_origin_inside_cell() {
        let v = vec![Voxel {
            linear_rgb: None,
            center: [-1.5, 0.5, 0.5],
            evidence: 1.,
        }];
        assert_eq!(
            raycast(&v, 1., [0., 0.5, 0.5], [-1., 0., 0.], 10.)
                .unwrap()
                .unwrap()
                .distance,
            1.
        );
        assert_eq!(
            raycast(&v, 1., [-1.5, 0.5, 0.5], [0., 1., 0.], 10.)
                .unwrap()
                .unwrap()
                .distance,
            0.
        );
    }
}
