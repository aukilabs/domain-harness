//! Compact pose profile over SDK Outputs, Products and Registry v3.
//! This is an application profile, not a silent change to SDK odometry v1.
use anyhow::{Result, ensure};
use auki_components::{ClockReference, ContractType, OutputManifest, PayloadContract};
use auki_registry::FrameRegistryEntry;
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "spatial-harness.compact-pose/v1";
pub const METADATA: &str = "spatial-harness.compact-pose-contract/v1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameRef { pub peer_id: String, pub id: String, pub hash: String }
impl From<&FrameRegistryEntry> for FrameRef {
    fn from(f: &FrameRegistryEntry)->Self {Self {peer_id:f.peer_id.clone(),id:f.frame_id.clone(),hash:f.hash()}}
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub from_frame: FrameRef,
    pub to_frame: FrameRef,
    pub clock: ClockReference,
    pub session_id: String,
}
impl Contract {
    // PR425 has no generic contract-reference field. Bind the immutable
    // descriptor into output_id, which is itself covered by the SDK manifest hash.
    pub fn output_id(&self)->String {format!("poses-{}",auki_components::manifest_hash(self))}
    pub fn validate_output(&self, output:&OutputManifest)->Result<()> {
        output.validate().map_err(anyhow::Error::msg)?;
        ensure!(!self.session_id.trim().is_empty() && self.session_id.len()<=256,"Invalid pose session");
        ensure!(self.from_frame!=self.to_frame,"Pose endpoints must differ");
        ensure!(self.from_frame.peer_id==output.peer_id && self.to_frame.peer_id==output.peer_id
            && self.clock.peer_id==output.peer_id,"Pose registry owner mismatch");
        ensure!(output.output_id==self.output_id() && output.clock==self.clock,"Pose output contract mismatch");
        ensure!(matches!(&output.payload,PayloadContract::Structured(p) if p.datatype==Update::DATATYPE && p.schema==SCHEMA),"Wrong compact pose type");
        ensure!(output.spatial_frame.as_ref().is_some_and(|f|FrameRef::from(&f.definition)==self.to_frame),"Pose destination mismatch");
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update { pub tracking: auki_odometry::Tracking }
impl ContractType for Update {const DATATYPE:&'static str=SCHEMA;}
impl Update {
    pub fn validate(&self)->Result<()> {
        if let auki_odometry::Tracking::Valid{pose}= &self.tracking {pose.validate()?;}
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn samples_have_no_definitions_or_references() {
        let lost=Update{tracking:auki_odometry::Tracking::Lost};
        assert_eq!(serde_json::to_string(&lost).unwrap(),r#"{"tracking":{"status":"lost"}}"#);
        assert!(serde_json::from_str::<Update>(r#"{"tracking":{"status":"lost"},"contract":{}}"#).is_err());
    }
    #[test]
    fn changing_a_reference_changes_output_identity() {
        let mut c=Contract{from_frame:FrameRef{peer_id:"p".into(),id:"base".into(),hash:"a".into()},
            to_frame:FrameRef{peer_id:"p".into(),id:"world".into(),hash:"b".into()},
            clock:ClockReference{peer_id:"p".into(),id:"clock".into(),hash:"c".into()},session_id:"session".into()};
        let before=c.output_id();c.to_frame.hash="new".into();assert_ne!(before,c.output_id());
    }
    #[test]
    fn rejects_mutated_descriptor_and_wrong_output_clock() {
        use auki_components::*;
        use auki_registry::CoordinateConvention;
        let from=FrameRegistryEntry::in_convention("peer","base",CoordinateConvention::ZUpRightForward);
        let to=FrameRegistryEntry::in_convention("peer","world",CoordinateConvention::ZUpRightForward);
        let c=Contract{from_frame:(&from).into(),to_frame:(&to).into(),
            clock:ClockReference{peer_id:"peer".into(),id:"clock".into(),hash:"a".repeat(32)},session_id:"session".into()};
        let runtime=ComponentRuntime::new("peer");
        let component=runtime.component(ComponentSpec::new("pose").observable(ObservableContract{
            name:"poses".into(),datatype:SCHEMA.into(),schema:SCHEMA.into(),
            access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster})).unwrap();
        let output:ConfiguredObservable<Update>=component.configured_observable(
            ConfiguredObservableSpec::new("poses",c.output_id(),c.clock.clone(),
            PayloadContract::Structured(StructuredPayloadContract{modality:"pose".into(),datatype:SCHEMA.into(),
                schema:SCHEMA.into(),observes:"robot_base_pose_in_odometry".into(),unit:None})).in_registered_frame(to)).unwrap();
        let manifest=output.manifest();
        c.validate_output(&manifest).unwrap();
        let mut changed=c.clone();changed.from_frame.hash="b".repeat(32);
        assert!(changed.validate_output(&manifest).is_err());
        let mut manifest=manifest.clone();manifest.clock.hash="c".repeat(32);
        assert!(c.validate_output(&manifest).is_err());
    }
    #[test]
    fn rejects_invalid_transform_and_keeps_valid_payload_small() {
        let mut update=Update{tracking:auki_odometry::Tracking::Valid{pose:auki_odometry::Pose{
            translation:[1.,2.,3.],rotation_xyzw:[0.,0.,0.,1.]}}};
        update.validate().unwrap();
        let json=serde_json::to_string(&update).unwrap();
        assert!(json.len()<150);
        assert!(!json.contains("frame") && !json.contains("clock") && !json.contains("session"));
        if let auki_odometry::Tracking::Valid{pose}= &mut update.tracking {pose.rotation_xyzw=[0.;4];}
        assert!(update.validate().is_err());
    }
}
