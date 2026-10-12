//! A phone reports a Portal the scene does not know yet. Home validates the
//! phone's alignment evidence exactly as it does for depth, composes the new
//! Portal's pose itself, and adds it to the selected scene.
//!
//! The same report with `replace` moves a Portal the scene already knows, for
//! when someone has moved the printed target. The phone must be aligned through
//! a different Portal, and the scene's origin Portal stays where it is.
use crate::local_mapper::{compose, from_rigid, inverse, rigid};
use anyhow::{Result, anyhow, ensure};
use auki_components::ContractType;
use auki_scenegraph::{MapSnapshot, QrAnchor, RigidTransform};
use serde::{Deserialize, Serialize};

pub const COMPONENT: &str = "home-phone-portals";
/// Size assumed for a newly discovered Portal: the encoded square, in metres.
pub const DEFAULT_SIDE_M: f64 = 0.15;
pub const MAX_PORTALS: usize = 64;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub session: String,
    pub timestamp_ns: u64,
    /// The eleven-character Portal code read from the QR payload.
    pub code: String,
    pub side_length_m: f64,
    /// Measured pose of the new Portal in the phone's AR session.
    pub portal_to_session: RigidTransform,
    /// The known-Portal observation that ties that session to the scene.
    pub alignment: crate::mobile_depth::Alignment,
    /// Move a Portal the scene already has, instead of adding a new one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
}
impl ContractType for Report {
    const DATATYPE: &'static str = "spatial.phone-portal-report/v1";
}

fn valid_pose(p: &RigidTransform) -> bool {
    p.translation
        .iter()
        .chain(p.rotation_wxyz.iter())
        .all(|v| v.is_finite())
        && (p.rotation_wxyz.iter().map(|v| v * v).sum::<f64>() - 1.).abs() < 0.001
}

pub fn local_frame(code: &str) -> String {
    format!("portal:{code}:local")
}

/// Validate a report against the scene and return the anchor to add or move.
pub fn anchor(report: &Report, scene: &MapSnapshot) -> Result<QrAnchor> {
    uuid::Uuid::parse_str(&report.session)?;
    let session = format!("phone-ar:{}", report.session);
    let frame = &scene.scenegraph.map.frame.id;
    ensure!(
        report.code.len() == 11
            && report
                .code
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
        "Invalid Portal code"
    );
    let existing = scene.scenegraph.anchors.get(&report.code);
    if report.replace {
        ensure!(existing.is_some(), "Portal is not in this scene, so it cannot be moved");
        // The scene's frame is named after its origin Portal; everything else is placed from it.
        ensure!(
            !frame.ends_with(&format!(":portal-{}", report.code)),
            "This Portal is the scene's origin and cannot be moved"
        );
        ensure!(
            report.alignment.portal_id != report.code,
            "A Portal cannot be moved using itself as the reference"
        );
    } else {
        ensure!(existing.is_none(), "Portal is already in this scene");
        ensure!(
            scene.scenegraph.anchors.len() < MAX_PORTALS,
            "Scene Portal capacity reached"
        );
    }
    ensure!(
        report.side_length_m.is_finite() && (0.02..=2.).contains(&report.side_length_m),
        "Invalid Portal size"
    );
    let a = &report.alignment;
    ensure!(
        a.timestamp_ns > 0 && report.timestamp_ns >= a.timestamp_ns,
        "Portal report predates calibration"
    );
    ensure!(
        report.portal_to_session.from_frame_id == local_frame(&report.code)
            && report.portal_to_session.to_frame_id == session
            && a.portal_to_session.to_frame_id == session
            && a.session_to_home.from_frame_id == session
            && a.session_to_home.to_frame_id == *frame,
        "Portal report transform endpoints mismatch"
    );
    let known = scene
        .scenegraph
        .anchors
        .get(&a.portal_id)
        .ok_or_else(|| anyhow!("Unknown calibration Portal"))?;
    ensure!(
        a.portal_to_session.from_frame_id == known.pose_in_map.from_frame_id,
        "Calibration Portal frame mismatch"
    );
    ensure!(
        valid_pose(&report.portal_to_session)
            && valid_pose(&a.portal_to_session)
            && valid_pose(&a.session_to_home),
        "Invalid Portal report transform"
    );
    // Same consistency rule as phone depth: the claimed session-to-scene
    // transform must follow from the known Portal's observation.
    let expected = compose(
        &from_rigid(&known.pose_in_map),
        &inverse(&from_rigid(&a.portal_to_session))?,
    )?;
    let delta = compose(&inverse(&expected)?, &from_rigid(&a.session_to_home))?;
    ensure!(
        delta.translation.iter().map(|v| v * v).sum::<f64>() < 0.0001
            && delta.rotation_xyzw[3].abs() > 0.9999,
        "Portal report alignment disagrees with its calibration Portal"
    );
    let pose = compose(&expected, &from_rigid(&report.portal_to_session))?;
    ensure!(
        pose.translation.iter().map(|v| v * v).sum::<f64>() <= 100. * 100.,
        "Portal is implausibly far from the scene origin"
    );
    // A moved Portal keeps its identity and printed size; only its place changes.
    let (payload, side_length_m, local) = match existing {
        Some(known) => (known.payload.clone(), known.side_length_m, known.pose_in_map.from_frame_id.clone()),
        None => (report.code.clone(), report.side_length_m, local_frame(&report.code)),
    };
    Ok(QrAnchor {
        anchor_id: report.code.clone(),
        payload,
        side_length_m,
        pose_in_map: rigid(&pose, &local, frame),
    })
}

/// Send one report to Home's Portal receiver for the paired scene.
pub async fn send(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
    report: &Report,
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
            .find(|c| c.manifest.component_id == home.scoped_component(COMPONENT))
            .ok_or_else(|| anyhow!("Home Portal receiver unavailable"))?;
        let response = client
            .invoke_exact::<_, bool>(
                expected,
                route,
                component.manifest.reference(),
                "add",
                "phone-portal",
                uuid::Uuid::new_v4().to_string(),
                Some(Duration::from_secs(3)),
                report,
            )
            .await?;
        return Ok(response.result);
    }
    anyhow::bail!("Home Portal receiver unreachable")
}
