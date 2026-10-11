//! Home-owned upright semantic boxes, in the containing scene's metric frame.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

// Unknown fields are accepted on purpose: a newer Home adding a field to a
// volume must not make every older phone reject the whole scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SemanticVolume {
    pub id: String,
    pub name: String,
    pub semantic_type: String,
    /// Free-text instructions or notes for the region, when Home has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Bottom centre; dimensions are local X/Y/Z, yaw is about scene +Y.
    pub position: [f64; 3],
    pub dimensions: [f64; 3],
    pub yaw: f64,
}
impl SemanticVolume {
    pub fn validate(&self) -> Result<()> {
        uuid::Uuid::parse_str(&self.id)?;
        for (value, max) in [(&self.name, 120), (&self.semantic_type, 80)] {
            ensure!(!value.trim().is_empty() && value.chars().count() <= max
                && !value.chars().any(char::is_control), "Invalid volume name or type");
        }
        ensure!(self.position.iter().all(|x| x.is_finite() && x.abs() < 1_000_000.),
            "Invalid volume position");
        ensure!(self.dimensions.iter().all(|x| x.is_finite() && (0.05..=40.).contains(x)),
            "Volume dimensions must be 0.05–40 metres");
        ensure!(self.description.as_ref().is_none_or(|d| d.chars().count() <= 10_000),
            "Volume description is too long");
        ensure!(self.yaw.is_finite() && self.yaw.abs() <= std::f64::consts::PI + 1e-6,
            "Invalid volume yaw");
        Ok(())
    }
}
