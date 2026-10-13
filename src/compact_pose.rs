//! Compact pose profile over SDK Outputs, Products and Registry v3.
//! This is an application profile, not a silent change to SDK odometry v1.
//!
//! It is the one contract for any transform a peer knows between two frames. A
//! pose is a transform that changes often; how a peer's frame sits in another
//! peer's frame is one that changes rarely. A peer's frame registry is its
//! Registry v3 definitions plus the Products under this profile in its Catalog:
//! to ask whether a peer knows A-space to B-space, look there for a contract
//! from A's frame to B's.
use anyhow::{Result, ensure};
use auki_components::{ClockReference, ContractType, OutputManifest, PayloadContract};
use auki_registry::FrameRegistryEntry;
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "spatial-harness.compact-pose/v1";
pub const METADATA: &str = "spatial-harness.compact-pose-contract/v1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameRef {
    pub peer_id: String,
    pub id: String,
    pub hash: String,
}
impl From<&FrameRegistryEntry> for FrameRef {
    fn from(f: &FrameRegistryEntry) -> Self {
        Self {
            peer_id: f.peer_id.clone(),
            id: f.frame_id.clone(),
            hash: f.hash(),
        }
    }
}
/// How long one observation of a transform speaks for. Declared, so a reader
/// never infers it from a Product's name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifetime {
    /// A measurement of something moving: true at its timestamp. A reader
    /// interpolates between neighbours and treats an old one as stale.
    #[default]
    Sampled,
    /// A relationship that stands until it is replaced or declared lost, such
    /// as where one frame sits in another. It does not go stale with age.
    Held,
}
impl Lifetime {
    fn is_sampled(&self) -> bool {
        *self == Self::Sampled
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub from_frame: FrameRef,
    pub to_frame: FrameRef,
    pub clock: ClockReference,
    pub session_id: String,
    /// Omitted when sampled, so a pose contract is unchanged on the wire.
    #[serde(default, skip_serializing_if = "Lifetime::is_sampled")]
    pub lifetime: Lifetime,
}
impl Contract {
    // PR425 has no generic contract-reference field. Bind the immutable
    // descriptor into output_id, which is itself covered by the SDK manifest hash.
    pub fn output_id(&self) -> String {
        format!("poses-{}", auki_components::manifest_hash(self))
    }
    pub fn validate_output(&self, output: &OutputManifest) -> Result<()> {
        output.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            !self.session_id.trim().is_empty() && self.session_id.len() <= 256,
            "Invalid pose session"
        );
        ensure!(
            self.from_frame != self.to_frame,
            "Pose endpoints must differ"
        );
        // A peer measures from a frame of its own, on a clock of its own. The
        // destination may be another peer's frame: that is how a peer says where
        // its space sits in someone else's. Its definition is that peer's to serve.
        ensure!(
            self.from_frame.peer_id == output.peer_id && self.clock.peer_id == output.peer_id,
            "Pose registry owner mismatch"
        );
        ensure!(
            output.output_id == self.output_id() && output.clock == self.clock,
            "Pose output contract mismatch"
        );
        ensure!(
            matches!(&output.payload,PayloadContract::Structured(p) if p.datatype==Update::DATATYPE && p.schema==SCHEMA),
            "Wrong compact pose type"
        );
        ensure!(
            output
                .spatial_frame
                .as_ref()
                .is_some_and(|f| FrameRef::from(&f.definition) == self.to_frame),
            "Pose destination mismatch"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub tracking: auki_odometry::Tracking,
}
impl ContractType for Update {
    const DATATYPE: &'static str = SCHEMA;
}
impl Update {
    pub fn validate(&self) -> Result<()> {
        if let auki_odometry::Tracking::Valid { pose } = &self.tracking {
            pose.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn samples_have_no_definitions_or_references() {
        let lost = Update {
            tracking: auki_odometry::Tracking::Lost,
        };
        assert_eq!(
            serde_json::to_string(&lost).unwrap(),
            r#"{"tracking":{"status":"lost"}}"#
        );
        assert!(
            serde_json::from_str::<Update>(r#"{"tracking":{"status":"lost"},"contract":{}}"#)
                .is_err()
        );
    }
    #[test]
    fn changing_a_reference_changes_output_identity() {
        let mut c = Contract {
            from_frame: FrameRef {
                peer_id: "p".into(),
                id: "base".into(),
                hash: "a".into(),
            },
            to_frame: FrameRef {
                peer_id: "p".into(),
                id: "world".into(),
                hash: "b".into(),
            },
            clock: ClockReference {
                peer_id: "p".into(),
                id: "clock".into(),
                hash: "c".into(),
            },
            session_id: "session".into(),
            lifetime: Lifetime::Sampled,
        };
        let before = c.output_id();
        c.to_frame.hash = "new".into();
        assert_ne!(before, c.output_id());
    }
    #[test]
    fn rejects_mutated_descriptor_and_wrong_output_clock() {
        use auki_components::*;
        use auki_registry::CoordinateConvention;
        let from = FrameRegistryEntry::in_convention(
            "peer",
            "base",
            CoordinateConvention::ZUpRightForward,
        );
        let to = FrameRegistryEntry::in_convention(
            "peer",
            "world",
            CoordinateConvention::ZUpRightForward,
        );
        let c = Contract {
            from_frame: (&from).into(),
            to_frame: (&to).into(),
            clock: ClockReference {
                peer_id: "peer".into(),
                id: "clock".into(),
                hash: "a".repeat(32),
            },
            session_id: "session".into(),
            lifetime: Lifetime::Sampled,
        };
        let runtime = ComponentRuntime::new("peer");
        let component = runtime
            .component(ComponentSpec::new("pose").observable(ObservableContract {
                name: "poses".into(),
                datatype: SCHEMA.into(),
                schema: SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }))
            .unwrap();
        let output: ConfiguredObservable<Update> = component
            .configured_observable(
                ConfiguredObservableSpec::new(
                    "poses",
                    c.output_id(),
                    c.clock.clone(),
                    PayloadContract::Structured(StructuredPayloadContract {
                        modality: "pose".into(),
                        datatype: SCHEMA.into(),
                        schema: SCHEMA.into(),
                        observes: "robot_base_pose_in_odometry".into(),
                        unit: None,
                    }),
                )
                .in_registered_frame(to),
            )
            .unwrap();
        let manifest = output.manifest();
        c.validate_output(&manifest).unwrap();
        let mut changed = c.clone();
        changed.from_frame.hash = "b".repeat(32);
        assert!(changed.validate_output(&manifest).is_err());
        let mut manifest = manifest.clone();
        manifest.clock.hash = "c".repeat(32);
        assert!(c.validate_output(&manifest).is_err());
    }
    #[test]
    fn a_peer_may_relate_its_frame_to_another_peers_but_not_measure_from_one() {
        use auki_components::*;
        use auki_registry::CoordinateConvention;
        let mine =
            FrameRegistryEntry::in_convention("peer", "session", CoordinateConvention::OpenGl);
        let theirs =
            FrameRegistryEntry::in_convention("other", "scene", CoordinateConvention::OpenGl);
        let c = Contract {
            from_frame: (&mine).into(),
            to_frame: (&theirs).into(),
            clock: ClockReference {
                peer_id: "peer".into(),
                id: "clock".into(),
                hash: "a".repeat(32),
            },
            session_id: "session".into(),
            lifetime: Lifetime::Sampled,
        };
        let runtime = ComponentRuntime::new("peer");
        let component = runtime
            .component(
                ComponentSpec::new("transforms").observable(ObservableContract {
                    name: "transform".into(),
                    datatype: SCHEMA.into(),
                    schema: SCHEMA.into(),
                    access: vec![ObservationAccess::FollowNew],
                    exposure: Exposure::Cluster,
                }),
            )
            .unwrap();
        let output: ConfiguredObservable<Update> = component
            .configured_observable(
                ConfiguredObservableSpec::new(
                    "transform",
                    c.output_id(),
                    c.clock.clone(),
                    PayloadContract::Structured(StructuredPayloadContract {
                        modality: "pose".into(),
                        datatype: SCHEMA.into(),
                        schema: SCHEMA.into(),
                        observes: "session_in_scene".into(),
                        unit: None,
                    }),
                )
                .in_registered_frame(theirs.clone()),
            )
            .unwrap();
        c.validate_output(&output.manifest()).unwrap();
        let backwards = Contract {
            from_frame: (&theirs).into(),
            to_frame: (&mine).into(),
            ..c.clone()
        };
        assert!(backwards.validate_output(&output.manifest()).is_err());
    }
    #[test]
    fn lifetime_is_declared_and_a_sampled_contract_is_unchanged_on_the_wire() {
        let frame = |id: &str| FrameRef {
            peer_id: "p".into(),
            id: id.into(),
            hash: "a".into(),
        };
        let sampled = Contract {
            from_frame: frame("camera"),
            to_frame: frame("session"),
            clock: ClockReference {
                peer_id: "p".into(),
                id: "clock".into(),
                hash: "c".into(),
            },
            session_id: "s".into(),
            lifetime: Lifetime::Sampled,
        };
        let json = serde_json::to_string(&sampled).unwrap();
        assert!(!json.contains("lifetime"), "{json}");
        assert_eq!(serde_json::from_str::<Contract>(&json).unwrap(), sampled);
        let held = Contract {
            lifetime: Lifetime::Held,
            ..sampled.clone()
        };
        assert!(
            serde_json::to_string(&held)
                .unwrap()
                .contains(r#""lifetime":"held""#)
        );
        assert_ne!(
            held.output_id(),
            sampled.output_id(),
            "how long a transform holds is part of what it is"
        );
    }
    #[test]
    fn rejects_invalid_transform_and_keeps_valid_payload_small() {
        let mut update = Update {
            tracking: auki_odometry::Tracking::Valid {
                pose: auki_odometry::Pose {
                    translation: [1., 2., 3.],
                    rotation_xyzw: [0., 0., 0., 1.],
                },
            },
        };
        update.validate().unwrap();
        let json = serde_json::to_string(&update).unwrap();
        assert!(json.len() < 150);
        assert!(!json.contains("frame") && !json.contains("clock") && !json.contains("session"));
        if let auki_odometry::Tracking::Valid { pose } = &mut update.tracking {
            pose.rotation_xyzw = [0.; 4];
        }
        assert!(update.validate().is_err());
    }
}
