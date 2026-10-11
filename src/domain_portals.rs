//! Offline import of user-supplied domain poses. The synthetic scene is authored
//! directly in this frame by user request. Raw rounded values remain in config.
use crate::portal_sizes::PortalSizes;
use anyhow::{Result, ensure};
use auki_scenegraph::{
    CoordinateConvention, MapDefinition, MapFrame, MapSnapshot, QrAnchor, RigidTransform,
    Scenegraph,
};
use serde::Deserialize;

pub const DOMAIN: &str = "b2881524-954d-4979-b715-240ed8f02807";
pub const FRAME: &str = "domain:b2881524-954d-4979-b715-240ed8f02807:map";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainPortals {
    pub schema: u32,
    pub domain_id: String,
    pub convention: String,
    pub units: String,
    pub transform_direction: String,
    pub quaternion_order: String,
    pub rounded: bool,
    pub height_note: String,
    pub portals: Vec<PortalPose>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortalPose {
    pub code: String,
    pub position: [f64; 3],
    pub rotation: [f64; 4],
}
impl DomainPortals {
    pub fn snapshot(&self, sizes: &PortalSizes) -> Result<MapSnapshot> {
        ensure!(
            self.schema == 1
                && self.domain_id == DOMAIN
                && self.convention == "opengl"
                && self.units == "meters"
                && self.transform_direction == "portal_local_to_domain_map"
                && self.quaternion_order == "xyzw",
            "unsupported domain pose contract"
        );
        let mut scene = Scenegraph::new(MapDefinition {
            map_id: format!("domain:{}", self.domain_id),
            name: Some(crate::environments::reference_name().into()),
            domain_reference: Some(self.domain_id.clone()),
            frame: MapFrame::in_convention(
                FRAME,
                "User-supplied domain-map origin; synthetic content is authored in this frame",
                CoordinateConvention::OpenGl,
                1.,
            )?,
        })?;
        for p in &self.portals {
            ensure!(
                p.position
                    .iter()
                    .chain(p.rotation.iter())
                    .all(|v| v.is_finite()),
                "non-finite Portal pose"
            );
            let norm = p.rotation.iter().map(|v| v * v).sum::<f64>().sqrt();
            ensure!(
                (norm - 1.).abs() < 0.001,
                "Portal quaternion is not a rounded unit rotation"
            );
            let [x, y, z, w] = p.rotation.map(|v| v / norm);
            let size = sizes
                .encoded_size_m(&p.code)?
                .ok_or_else(|| anyhow::anyhow!("missing manual size for {}", p.code))?;
            let anchor = QrAnchor {
                anchor_id: p.code.clone(),
                payload: p.code.clone(),
                side_length_m: size,
                pose_in_map: RigidTransform {
                    from_frame_id: format!("portal:{}:local", p.code),
                    to_frame_id: FRAME.into(),
                    translation: p.position,
                    rotation_wxyz: [w, x, y, z],
                },
            };
            ensure!(
                scene.anchors.insert(p.code.clone(), anchor).is_none(),
                "duplicate Portal code"
            );
        }
        MapSnapshot::new(scene).map_err(Into::into)
    }
}
pub fn load() -> Result<MapSnapshot> {
    let poses: DomainPortals =
        serde_json::from_slice(&std::fs::read("config/domain-portals.json")?)?;
    poses.snapshot(&PortalSizes::load("config/portals.json")?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stored_poses_preserve_positions_sizes_frames_and_quaternion_direction() {
        let source: DomainPortals =
            serde_json::from_str(include_str!("../config/domain-portals.json")).unwrap();
        let sizes: PortalSizes =
            serde_json::from_str(include_str!("../config/portals.json")).unwrap();
        let map = source.snapshot(&sizes).unwrap();
        map.validate().unwrap();
        assert_eq!(map.scenegraph.anchors.len(), 13);
        assert_eq!(map.scenegraph.map.domain_reference.as_deref(), Some(DOMAIN));
        for p in &source.portals {
            let a = &map.scenegraph.anchors[&p.code];
            assert_eq!(a.pose_in_map.translation, p.position);
            assert_eq!(a.pose_in_map.translation[1], 0.);
            assert_eq!(a.pose_in_map.to_frame_id, FRAME);
            assert_eq!(
                Some(a.side_length_m),
                sizes.encoded_size_m(&p.code).unwrap()
            );
            let norm = p.rotation.iter().map(|v| v * v).sum::<f64>().sqrt();
            let q = a.pose_in_map.rotation_wxyz;
            for (actual, expected) in
                q.into_iter()
                    .zip([p.rotation[3], p.rotation[0], p.rotation[1], p.rotation[2]])
            {
                assert!((actual - expected / norm).abs() < 1e-12);
            }
            // A local +Z normal transforms upward for these stored orientations.
            assert!((2. * (q[2] * q[3] - q[0] * q[1]) - 1.).abs() < 0.001);
        }
    }
}
