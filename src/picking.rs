//! Bounded, short-lived handles bind camera clicks to the displayed calibrated
//! frame and save clicks to a server-computed Home voxel contact.
use crate::{network::CanonicalScene, notes::SurfaceAnchor};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
#[derive(Clone, Deserialize)]
pub struct CameraGeometry {
    pub frame: String,
    pub camera_to_map: auki_odometry::Pose,
    pub calibration: Value,
}
#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum PickRequest {
    Scene {
        frame: String,
        origin: [f64; 3],
        direction: [f64; 3],
    },
    Camera {
        camera_token: String,
        pixel: [f64; 2],
    },
}
#[derive(Clone)]
pub struct Contact {
    pub position: [f64; 3],
    pub anchor: SurfaceAnchor,
}
#[derive(Default)]
pub struct Picks {
    cameras: VecDeque<(String, Instant, CameraGeometry)>,
    hits: VecDeque<(String, Instant, Contact)>,
}
impl Picks {
    pub fn pick_canonical(
        &mut self,
        origin: [f64; 3],
        direction: [f64; 3],
        voxels: &[crate::raycast::Voxel],
        size: f64,
    ) -> Result<Value> {
        self.pick_frame(origin,direction,voxels,size,crate::model::FRAME)
    }
    pub fn pick_frame(&mut self, origin:[f64;3], direction:[f64;3], voxels:&[crate::raycast::Voxel], size:f64, frame:&str) -> Result<Value> {
        let Some((hit, floor)) =
            crate::mobile_notes::intersection(voxels, size, origin, direction)?
        else {
            return Ok(json!({"hit":null}));
        };
        let contact = Contact {
            position: hit.position,
            anchor: SurfaceAnchor {
                floor,
                frame: frame.into(),

                voxel_center: hit.center,
                beam_diameter_m: 0.,
            },
        };
        let token = uuid::Uuid::new_v4().to_string();
        if self.hits.len() >= 128 {
            self.hits.pop_front();
        }
        self.hits
            .push_back((token.clone(), Instant::now(), contact));
        Ok(json!({"hit":{"token":token,"position":hit.position,"floor":floor},"beam_diameter_m":0}))
    }
    pub fn remember_camera(&mut self, value: Value) -> Result<String> {
        let geometry: CameraGeometry = serde_json::from_value(value)?;
        geometry.camera_to_map.validate()?;
        let token = uuid::Uuid::new_v4().to_string();
        if self.cameras.len() >= 64 {
            self.cameras.pop_front();
        }
        self.cameras
            .push_back((token.clone(), Instant::now(), geometry));
        Ok(token)
    }
    pub fn pick(&mut self, request: PickRequest, scene: &CanonicalScene) -> Result<Value> {
        ensure!(
            scene.frame.starts_with("home-live:")
                && scene.registries["local_mapping"]["origin"]["kind"] == "canonical_portals",
            "Home live voxel map is not aligned"
        );
        let target = &scene.voxels.definition.frame;
        let home = crate::conventions::registry(&scene.authority_peer_id, crate::model::FRAME)?;
        let convert = |p: [f64; 3],
                       from: &auki_registry::FrameRegistryEntry,
                       to: &auki_registry::FrameRegistryEntry|
         -> Result<[f64; 3]> {
            let p = auki_geometry::convert_point_convention(
                auki_datatypes::pose::Vec3 {
                    x: p[0],
                    y: p[1],
                    z: p[2],
                },
                from,
                to,
            )?;
            Ok([p.x, p.y, p.z])
        };
        let (origin, direction) = match request {
            PickRequest::Scene {
                frame,
                origin,
                direction,
            } => {
                ensure!(
                    frame == crate::model::FRAME
                        || frame == crate::local_mapper::display_frame(&scene.frame)?,
                    "Select Home live map to place a label"
                );
                (
                    convert(origin, &home, target)?,
                    convert(direction, &home, target)?,
                )
            }
            PickRequest::Camera {
                camera_token,
                pixel,
            } => {
                let (_, received, g) =
                    self.cameras
                        .iter()
                        .find(|(id, _, _)| id == &camera_token)
                        .ok_or_else(|| anyhow!("Camera frame expired; try a fresh frame"))?;
                ensure!(
                    received.elapsed() < Duration::from_secs(15) && g.frame == scene.frame,
                    "Camera frame expired or belongs to an earlier Home map"
                );
                let w = g.calibration["width"].as_f64().unwrap_or(0.);
                let h = g.calibration["height"].as_f64().unwrap_or(0.);
                ensure!(
                    (0. ..w).contains(&pixel[0]) && (0. ..h).contains(&pixel[1]),
                    "Pixel outside camera image"
                );
                let ray = crate::ar_preview::pixel_ray(&g.calibration, pixel)?;
                let endpoint = crate::local_mapper::compose(
                    &g.camera_to_map,
                    &auki_odometry::Pose {
                        translation: ray,
                        rotation_xyzw: [0., 0., 0., 1.],
                    },
                )?;
                (
                    g.camera_to_map.translation,
                    std::array::from_fn(|a| {
                        endpoint.translation[a] - g.camera_to_map.translation[a]
                    }),
                )
            }
        };
        let Some(hit) = crate::raycast::beamcast(
            &crate::model::occupied(&scene.voxels)?,
            scene.voxels.definition.grid().voxel_size_m.0,
            origin,
            direction,
            40.,
            0.25,
        )?
        else {
            return Ok(json!({"hit":null,"beam_diameter_m":0.5}));
        };
        let contact = Contact {
            position: convert(hit.position, target, &home)?,
            anchor: SurfaceAnchor {
                floor: false,
                frame: scene.frame.clone(),

                voxel_center: hit.center,
                beam_diameter_m: 0.5,
            },
        };
        let token = uuid::Uuid::new_v4().to_string();
        if self.hits.len() >= 128 {
            self.hits.pop_front();
        }
        self.hits
            .push_back((token.clone(), Instant::now(), contact.clone()));
        Ok(
            json!({"hit":{"token":token,"position":contact.position,"frame":crate::model::FRAME,"distance_m":hit.distance},"beam_diameter_m":0.5}),
        )
    }
    pub fn contact(&self, token: &str) -> Result<Contact> {
        let (_, when, contact) = self
            .hits
            .iter()
            .find(|(id, _, _)| id == token)
            .ok_or_else(|| anyhow!("Placement expired; right-click again"))?;
        ensure!(
            when.elapsed() < Duration::from_secs(300),
            "Placement expired; right-click again"
        );
        Ok(contact.clone())
    }
    pub fn consume(&mut self, token: &str) {
        self.hits.retain(|(id, _, _)| id != token);
    }
}
