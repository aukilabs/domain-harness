//! Read-only calculations. Frame bindings and the captured camera are supplied by Home.
use crate::{chatbot::Viewer, semantic_volumes::Volume};
use anyhow::{Result, ensure};
use auki_datatypes::pose::{Quat, SpatialTransform, Vec3};
use serde::Deserialize;
use serde_json::{Value, json};
fn transform(v: &Volume) -> SpatialTransform {
    SpatialTransform {
        translation: Some(Vec3 {
            x: v.center[0],
            y: 0.,
            z: v.center[2],
        }),
        orientation: Some(Quat {
            x: 0.,
            y: (v.yaw / 2.).sin(),
            z: 0.,
            w: (v.yaw / 2.).cos(),
        }),
    }
}
fn point(t: &SpatialTransform, p: [f64; 3]) -> Result<[f64; 3]> {
    let local = SpatialTransform {
        translation: Some(Vec3 {
            x: p[0],
            y: p[1],
            z: p[2],
        }),
        orientation: None,
    };
    let result = auki_geometry::compose_spatial_transforms(&local, t)?
        .translation
        .unwrap();
    Ok([result.x, result.y, result.z])
}
fn camera(view: &Viewer) -> Result<[f64; 3]> {
    let p = view
        .camera_position
        .ok_or_else(|| anyhow::anyhow!("Captured camera missing; refresh Home"))?;
    ensure!(
        view.mode == "3d"
            && p.iter()
                .chain(view.screen_right.iter())
                .all(|n| n.is_finite()),
        "Valid 3D viewpoint required"
    );
    Ok(p)
}
fn corners(v: &Volume) -> Result<Vec<(String, [f64; 3], [f64; 3])>> {
    ensure!(
        v.center.iter().chain(v.size.iter()).all(|n| n.is_finite())
            && v.size.iter().all(|n| *n > 0.)
            && v.yaw.is_finite(),
        "Invalid volume geometry"
    );
    let mut out = vec![];
    for x in [-1., 1.] {
        for z in [-1., 1.] {
            let local = [x * v.size[0] / 2., 0., z * v.size[2] / 2.];
            out.push((
                format!(
                    "x{}:z{}",
                    if x < 0. { "-" } else { "+" },
                    if z < 0. { "-" } else { "+" }
                ),
                point(&transform(v), local)?,
                local,
            ));
        }
    }
    Ok(out)
}
fn selected(v: &Volume, view: &Viewer, selector: &str) -> Result<(String, [f64; 3], [f64; 3])> {
    let camera = camera(view)?;
    let mut points = corners(v)?;
    if selector.starts_with('x') {
        return points
            .into_iter()
            .find(|p| p.0 == selector)
            .ok_or_else(|| anyhow::anyhow!("Unknown corner ID"));
    }
    ensure!(
        matches!(selector, "far" | "near" | "left" | "right"),
        "Unknown corner selector"
    );
    if matches!(selector, "left" | "right") {
        ensure!(
            view.screen_right[0].hypot(view.screen_right[2]) > 1e-6,
            "Viewer has no horizontal right direction"
        );
    }
    let score = |p: &(String, [f64; 3], [f64; 3])| {
        let distance = (p.1[0] - camera[0]).hypot(p.1[2] - camera[2]);
        let primary = match selector {
            "near" => -distance,
            "far" => distance,
            "left" => {
                -((p.1[0] - v.center[0]) * view.screen_right[0]
                    + (p.1[2] - v.center[2]) * view.screen_right[2])
            }
            _ => {
                (p.1[0] - v.center[0]) * view.screen_right[0]
                    + (p.1[2] - v.center[2]) * view.screen_right[2]
            }
        };
        (primary, distance)
    };
    points.sort_by(|a, b| {
        let a = score(a);
        let b = score(b);
        a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
    });
    Ok(points.pop().unwrap())
}
pub fn get_volume_corners(v: &Volume, view: &Viewer, frame: &str) -> Result<Value> {
    let mut labels = serde_json::Map::new();
    for label in ["far", "near", "left", "right"] {
        labels.insert(label.into(), json!(selected(v, view, label)?.0));
    }
    Ok(
        json!({"frame":frame,"ground_y":0,"volume_id":v.id,"corners":corners(v)?.iter().map(|p|json!({"id":p.0,"position":p.1})).collect::<Vec<_>>(),"labels":labels,"side_tie_break":"farther from camera"}),
    )
}
pub fn point_inside_volume(
    v: &Volume,
    view: &Viewer,
    selector: &str,
    distance: f64,
    mode: &str,
    frame: &str,
) -> Result<Value> {
    ensure!(
        distance.is_finite() && distance >= 0.,
        "Invalid inset distance"
    );
    let (id, _, mut p) = selected(v, view, selector)?;
    match mode {
        "toward_center" => {
            let length = p[0].hypot(p[2]);
            ensure!(
                distance <= length,
                "Inset exceeds distance to volume center"
            );
            if length > 0. {
                let scale = 1. - distance / length;
                p[0] *= scale;
                p[2] *= scale;
            }
        }
        "from_walls" => {
            ensure!(
                distance <= v.size[0] / 2. && distance <= v.size[2] / 2.,
                "Inset exceeds volume half-width/depth"
            );
            p[0] -= p[0].signum() * distance;
            p[2] -= p[2].signum() * distance;
        }
        _ => anyhow::bail!("Unknown inset mode"),
    }
    Ok(
        json!({"frame":frame,"volume_id":v.id,"corner_id":id,"position":point(&transform(v),p)?,"ground_y":0}),
    )
}
pub fn offset_point(
    mut position: [f64; 3],
    view: &Viewer,
    direction: &str,
    distance: f64,
    basis: &str,
    ground: bool,
    frame: &str,
) -> Result<Value> {
    ensure!(
        position.iter().all(|n| n.is_finite())
            && distance.is_finite()
            && distance >= 0.
            && distance <= 10000.,
        "Invalid offset"
    );
    ensure!(matches!(basis, "viewer" | "scene"), "Invalid basis");
    if ground {
        position[1] = 0.;
    }
    let mut delta = match direction {
        "toward_viewer" | "away_from_viewer" => {
            let p = camera(view)?;
            let sign = if direction == "toward_viewer" {
                1.
            } else {
                -1.
            };
            [
                (p[0] - position[0]) * sign,
                (p[1] - position[1]) * sign,
                (p[2] - position[2]) * sign,
            ]
        }
        "left" | "right" | "forward" | "backward" | "up" | "down" => {
            let local = match direction {
                "left" => [-1., 0., 0.],
                "right" => [1., 0., 0.],
                "forward" => [0., 0., -1.],
                "backward" => [0., 0., 1.],
                "up" => [0., 1., 0.],
                _ => [0., -1., 0.],
            };
            if basis == "scene" || matches!(direction, "up" | "down") {
                local
            } else {
                camera(view)?;
                let yaw = view.screen_right[2].atan2(view.screen_right[0]);
                let t = SpatialTransform {
                    translation: None,
                    orientation: Some(Quat {
                        x: 0.,
                        y: -(yaw / 2.).sin(),
                        z: 0.,
                        w: (yaw / 2.).cos(),
                    }),
                };
                point(&t, local)?
            }
        }
        _ => anyhow::bail!("Unknown offset direction"),
    };
    if ground {
        delta[1] = 0.;
    }
    let length = delta.iter().map(|n| n * n).sum::<f64>().sqrt();
    ensure!(
        length > 1e-9 || distance == 0.,
        "No direction on ground plane"
    );
    let distance = if direction == "toward_viewer" {
        distance.min(length)
    } else {
        distance
    };
    let translation = if length > 0. {
        delta.map(|n| n * distance / length)
    } else {
        [0.; 3]
    };
    let t = SpatialTransform {
        translation: Some(Vec3 {
            x: translation[0],
            y: translation[1],
            z: translation[2],
        }),
        orientation: None,
    };
    Ok(json!({"frame":frame,"position":point(&t,position)?,"ground":ground}))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corners {
    pub volume_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inside {
    pub volume_id: String,
    pub corner: String,
    pub distance_m: f64,
    pub mode: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offset {
    pub position: [f64; 3],
    pub direction: String,
    pub distance_m: f64,
    pub frame: String,
    pub ground: bool,
}
pub fn tools() -> Vec<Value> {
    serde_json::from_str(include_str!("../config/scene-geometry-tools.json"))
        .expect("validated geometry schemas")
}
/// Snap model-copied coordinates to the authoritative query result. One micron
/// tolerates JSON/decimal round trips without accepting a guessed destination.
pub fn resolve_calculated_point(requested:[f64;3],points:&[[f64;3]])->Result<[f64;3]>{
 ensure!(requested.iter().all(|n|n.is_finite()),"Invalid requested position");
 points.iter().find(|p|p.iter().zip(requested).all(|(a,b)|(*a-b).abs()<=1e-6)).copied().ok_or_else(||anyhow::anyhow!("Call a read-only geometry tool and use its returned position; do not guess coordinates"))
}
