//! Reference portals are aligned to measured portals, never used as observations.
use crate::local_mapper::{compose, from_rigid, inverse, rigid};
use anyhow::{Result, ensure};
use auki_scenegraph::{MapConventionConversion, MapFrame, MapSnapshot};
use serde_json::{Value, json};

pub struct HomePortals {
    pub portals: MapSnapshot,
    pub peer_id: String,
    pub revision: usize,
    pub anchor: String,
}
impl HomePortals {
    /// Convert axes with the SDK, then solve the independent physical origins.
    /// The caller selects the latest observed shared marker; its measured pose wins.
    pub fn align(&self, observed: &MapSnapshot) -> Result<(MapSnapshot, Value)> {
        let target = &observed.scenegraph.map.frame;
        let converted_frame = MapFrame::in_convention(
            "home-origin:runner-axes",
            "Home origin expressed in runner axes",
            target
                .convention
                .ok_or_else(|| anyhow::anyhow!("runner convention missing"))?,
            1.,
        )?;
        let converted = MapConventionConversion::between_named_frames(
            self.portals.scenegraph.map.frame.clone(),
            converted_frame,
        )?
        .convert_snapshot(&self.portals, "home-reference-axes")?;
        let home = converted
            .scenegraph
            .anchors
            .get(&self.anchor)
            .ok_or_else(|| anyhow::anyhow!("alignment portal missing from Home"))?;
        let measured = observed
            .scenegraph
            .anchors
            .get(&self.anchor)
            .ok_or_else(|| anyhow::anyhow!("alignment portal has not been observed"))?;
        ensure!(
            (home.side_length_m - measured.side_length_m).abs() < 1e-6,
            "alignment portal sizes differ"
        );
        let transform = compose(
            &from_rigid(&measured.pose_in_map),
            &inverse(&from_rigid(&home.pose_in_map))?,
        )?;
        let mut combined = observed.scenegraph.clone();
        let mut references = serde_json::Map::new();
        for a in converted.scenegraph.anchors.values() {
            let mut reference = a.clone();
            reference.anchor_id = format!("home:{}", a.anchor_id);
            reference.pose_in_map = rigid(
                &compose(&transform, &from_rigid(&a.pose_in_map))?,
                &a.pose_in_map.from_frame_id,
                &target.id,
            );
            references.insert(
                reference.anchor_id.clone(),
                json!({"source":"home","shortcode":a.anchor_id}),
            );
            ensure!(
                combined
                    .anchors
                    .insert(reference.anchor_id.clone(), reference)
                    .is_none(),
                "reference ID collision"
            );
        }
        let metadata = json!({"status":"aligned","anchor":self.anchor,"home_peer_id":self.peer_id,
            "home_revision":self.revision,"home_frame":self.portals.scenegraph.map.frame,
            "converted_home_frame":converted.scenegraph.map.frame,
            "runner_frame":target.id,"converted_home_to_runner":transform,
            "method":"SDK convention conversion, then latest shared-portal rigid alignment",
            "reference_portals":references});
        Ok((MapSnapshot::new(combined)?, metadata))
    }
}
