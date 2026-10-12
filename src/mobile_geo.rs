//! Where a scene sits on the Earth. A phone that found north with its compass
//! and then aligned to a Portal reports the coordinate of the scene's origin
//! and the direction of true north across its floor. Home keeps the best
//! position and the best north it has heard, saves them with the scene, and
//! publishes them, so the browser and every other phone share one compass.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub frame: String,
    /// Coordinate of the scene frame's origin, in degrees and metres above sea level.
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_m: f64,
    /// Radius the coordinate is good to, in metres.
    pub accuracy_m: f64,
    /// Unit direction of true north across the floor, as (X, Z) in the scene frame.
    pub north: [f64; 2],
    /// The compass's own error estimate when north was taken, in degrees.
    pub heading_error_deg: f64,
}
impl auki_components::ContractType for Reference {
    const DATATYPE: &'static str = "spatial.phone-geo/v1";
}

impl Reference {
    pub fn validate(&self, frame: &str) -> Result<()> {
        ensure!(self.frame == frame, "Geo reference belongs to another scene");
        ensure!(
            (-90. ..=90.).contains(&self.latitude)
                && (-180. ..=180.).contains(&self.longitude)
                && self.altitude_m.is_finite()
                && self.altitude_m.abs() <= 20_000.,
            "Invalid coordinate"
        );
        ensure!(
            (0. ..=10_000.).contains(&self.accuracy_m) && (0. ..=180.).contains(&self.heading_error_deg),
            "Invalid geo accuracy"
        );
        let length = self.north[0].hypot(self.north[1]);
        ensure!(length.is_finite() && (length - 1.).abs() < 0.01, "North is not a unit direction");
        Ok(())
    }
}

/// Fold a new report into what is known. Position and north are judged apart:
/// a phone with a good fix and a poor compass improves only the position.
/// Returns the new reference if anything changed.
pub fn merge(current: Option<&Reference>, report: &Reference) -> Option<Reference> {
    let Some(current) = current.filter(|c| c.frame == report.frame) else {
        return Some(report.clone());
    };
    let mut next = current.clone();
    if report.accuracy_m < current.accuracy_m {
        next.latitude = report.latitude;
        next.longitude = report.longitude;
        next.altitude_m = report.altitude_m;
        next.accuracy_m = report.accuracy_m;
    }
    if report.heading_error_deg < current.heading_error_deg {
        next.north = report.north;
        next.heading_error_deg = report.heading_error_deg;
    }
    (next != *current).then_some(next)
}

/// The saved reference for a scene, if it is still valid for this frame.
pub fn load(path: &Path, frame: &str) -> Option<Reference> {
    let bytes = std::fs::read(path).ok().filter(|b| b.len() <= 4096)?;
    let reference: Reference = serde_json::from_slice(&bytes).ok()?;
    reference.validate(frame).ok()?;
    Some(reference)
}

pub fn save(path: &Path, reference: &Reference) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(reference)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

/// Send one reference to Home's presence component for the paired scene.
pub async fn send(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
    reference: &Reference,
) -> Result<bool> {
    use auki_component_protocol::{CatalogResponse, ComponentProtocolClient};
    use std::time::Duration;
    let expected = home.validate(peer)?;
    let client = ComponentProtocolClient::new(peer.protocols());
    for route in &home.routes {
        let route: auki_sdk::Multiaddr = route.parse()?;
        let Ok(Ok(CatalogResponse::Snapshot { snapshot })) = tokio::time::timeout(
            Duration::from_secs(1),
            client.catalog_exact(expected, route.clone(), None),
        )
        .await
        else {
            continue;
        };
        let component = snapshot
            .components
            .iter()
            .find(|c| c.manifest.component_id == home.scoped_component(crate::mobile_pose::COMPONENT))
            .ok_or_else(|| anyhow::anyhow!("Home phone presence unavailable"))?;
        let response = client
            .invoke_exact::<_, bool>(
                expected,
                route,
                component.manifest.reference(),
                "geo",
                "phone-geo",
                uuid::Uuid::new_v4().to_string(),
                Some(Duration::from_secs(3)),
                reference,
            )
            .await?;
        return Ok(response.result);
    }
    anyhow::bail!("Home phone presence unreachable")
}
