//! Home chooses the first authenticated, valid observation accepted while its
//! reference is empty. A complete expected snapshot makes concurrent offers and
//! delayed offers from before a reset fail closed, without comparing peer clocks.
use anyhow::{ensure, Result};
use auki_components::ContractType;
use auki_scenegraph::{MapSnapshot, QrAnchor, RigidTransform};
use serde::{Deserialize, Serialize};

pub const OPERATION: &str = "establish_origin";
pub const DESCRIPTION: &str = "First observed Portal center is the origin at Y=0; printed face points +Y; Portal right is +X and top is -Z";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhoneMeasurement {
    pub code:String,pub side_length_m:f64,pub timestamp_ns:u64,
    pub portal_to_native:RigidTransform,pub expected_frame:String,pub expected_origin_description:String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub expected_scene: MapSnapshot,
    pub code: String,
    pub side_length_m: f64,
    pub timestamp_ns: u64,
    pub portal_to_native: RigidTransform,
}
impl ContractType for Observation {
    const DATATYPE: &'static str = "spatial.first-portal-observation/v1";
}

pub fn floor_anchor(code: &str, size: f64, frame: &str) -> Result<QrAnchor> {
    ensure!(code.len()==11 && code.bytes().all(|b|b.is_ascii_uppercase()||b.is_ascii_digit()), "Invalid Portal code");
    ensure!(size.is_finite() && (0.02..=2.).contains(&size), "Invalid Portal size");
    let h=std::f64::consts::FRAC_1_SQRT_2;
    Ok(QrAnchor{anchor_id:code.into(),payload:code.into(),side_length_m:size,
        pose_in_map:RigidTransform{from_frame_id:crate::mobile_portal::local_frame(code),to_frame_id:frame.into(),translation:[0.;3],rotation_wxyz:[h,-h,0.,0.]}})
}

/// The origin has a unique zero position. Other anchors cannot occupy that
/// same position; frame IDs are opaque and never used to choose the origin.
pub fn portal(scene: &MapSnapshot) -> Result<Option<&str>> {
    let mut zero=scene.scenegraph.anchors.values().filter(|a|a.pose_in_map.translation.iter().all(|v|v.abs()<1e-8));
    let first=zero.next();
    ensure!(zero.next().is_none(), "Scene has ambiguous origin Portals");
    ensure!(scene.scenegraph.anchors.is_empty() || first.is_some(), "Scene origin Portal missing");
    Ok(first.map(|a|a.anchor_id.as_str()))
}

pub fn establish(current: &MapSnapshot, observation: &Observation) -> Result<MapSnapshot> {
    current.validate()?;observation.expected_scene.validate()?;
    ensure!(current==&observation.expected_scene && current.scenegraph.anchors.is_empty(), "Scene already has an origin or was reset; fetch the current scene before observing another Portal");
    ensure!(observation.timestamp_ns>0, "Missing observation capture time");
    let pose=&observation.portal_to_native;
    ensure!(pose.from_frame_id==crate::mobile_portal::local_frame(&observation.code)
        && !pose.to_frame_id.is_empty() && pose.to_frame_id!=current.scenegraph.map.frame.id,
        "Observation must identify the measured Portal and its native frame");
    ensure!(pose.translation.iter().chain(pose.rotation_wxyz.iter()).all(|x|x.is_finite())
        && (pose.rotation_wxyz.iter().map(|x|x*x).sum::<f64>()-1.).abs()<0.001,"Invalid observed pose");
    let mut graph=current.scenegraph.clone();
    ensure!(crate::portal_mount::native_is_gravity_aligned(&pose.to_frame_id),
        "Observation frame has no known gravity alignment; origin classification deferred");
    let (mount,_)=crate::portal_mount::fit(pose.rotation_wxyz)?;
    ensure!(mount==crate::portal_mount::Mount::Floor,"First Portal must be flat on the floor, not on a wall");
    let anchor=floor_anchor(&observation.code,observation.side_length_m,&graph.map.frame.id)?;
    graph.anchors.insert(anchor.anchor_id.clone(),anchor);
    MapSnapshot::new(graph).map_err(Into::into)
}

pub fn reset(current: &MapSnapshot) -> Result<MapSnapshot> {
    let mut graph=current.scenegraph.clone();graph.anchors.clear();
    graph.map.frame.id=format!("local-scene:{}:epoch-{}",graph.map.map_id,uuid::Uuid::new_v4());
    graph.map.frame.origin_description=format!("{DESCRIPTION}. Waiting for observation; reset {}",uuid::Uuid::new_v4());
    MapSnapshot::new(graph).map_err(Into::into)
}

pub async fn send(peer:&auki_sdk::AukiPeer, home:&crate::network::HomeConnection, observation:&Observation)->Result<bool>{
    use auki_component_protocol::{CatalogResponse,ComponentProtocolClient};
    use std::time::Duration;
    let expected=home.validate(peer)?;
    ensure!(observation.expected_scene.scenegraph.map.frame.id==home.frame,"Observation belongs to another scene");
    let client=ComponentProtocolClient::new(peer.protocols());
    for route in &home.routes {
        let route:auki_sdk::Multiaddr=route.parse()?;
        let Ok(Ok(CatalogResponse::Snapshot{snapshot}))=tokio::time::timeout(Duration::from_secs(2),client.catalog_exact(expected,route.clone(),None)).await else {continue};
        let component=snapshot.components.iter().find(|c|c.manifest.component_id==home.scoped_component(crate::mobile_portal::COMPONENT)).ok_or_else(||anyhow::anyhow!("Home origin receiver unavailable"))?;
        return Ok(client.invoke_exact::<_,bool>(expected,route,component.manifest.reference(),OPERATION,"portal-observer",uuid::Uuid::new_v4().to_string(),Some(Duration::from_secs(3)),observation).await?.result);
    }
    anyhow::bail!("Home origin receiver unreachable")
}
