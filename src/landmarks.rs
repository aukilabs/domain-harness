//! Re-identifiable positions and full-pose anchors. Parenting is mapper policy.
//! This typed extension travels in the scene envelope until the pinned SDK's
//! QR-only scenegraph gains native generic nodes.
use anyhow::{Context, Result, ensure};
use auki_components::{ClockReference, ComponentReference, OutputReference};
use auki_scenegraph::{MapSnapshot, RigidTransform};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA: &str = "spatial.landmarks/v2";
pub const PORTAL: &str = "auki.landmark.portal";
pub const ARUCO: &str = "auki.landmark.aruco";
pub const APRILTAG: &str = "auki.landmark.apriltag";
pub const IMAGE: &str = "auki.landmark.image";
pub const CORNERS: [&str; 4] = ["top_left", "top_right", "bottom_right", "bottom_left"];
pub const OBSERVATIONS_SCHEMA: &str = "spatial.landmark-observations/v2";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageLandmark {
    pub recognition: Recognition,
    pub pixel: [f64; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LandmarkObservations {
    pub schema: String,
    pub detected_by: ComponentReference,
    pub source_observation: SourceObservation,
    pub landmarks: Vec<ImageLandmark>,
}
impl auki_components::ContractType for LandmarkObservations {
    const DATATYPE: &'static str = OBSERVATIONS_SCHEMA;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recognition {
    /// Extensible namespaced type shared by detectors and scene queries.
    pub landmark_type: String,
    /// Complete re-identification key, e.g. Portal shortcode/corner name.
    /// Independent of a particular scene node or component instance.
    pub identity: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceObservation {
    pub output: OutputReference,
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub clock: ClockReference,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_by: Option<ComponentReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapped_by: Option<ComponentReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_observation: Option<SourceObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_observation: Option<SourceObservation>,
}

/// Optional capability declaration keyed by the SDK's real component reference.
/// This is an extension, not a fabricated field in the SDK ComponentManifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentCapabilities {
    pub component: ComponentReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landmark_types: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub id: String,
    pub recognition: Recognition,
    pub pose: RigidTransform,
    #[serde(default)]
    pub provenance: Provenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Landmark {
    pub id: String,
    pub recognition: Recognition,
    pub position: [f64; 3],
    pub frame: String,
    /// Optional structural relationship selected by the mapper. With a parent,
    /// position is in that anchor's local frame; otherwise it is in map frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_anchor: Option<String>,
    #[serde(default)]
    pub provenance: Provenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Landmarks {
    pub schema: String,
    pub frame: String,
    pub anchors: BTreeMap<String, Anchor>,
    pub landmarks: BTreeMap<String, Landmark>,
    #[serde(default)]
    pub components: Vec<ComponentCapabilities>,
}

fn valid_text(text: &str) -> Result<()> {
    ensure!(
        !text.is_empty() && text.len() <= 1024 && !text.chars().any(char::is_control),
        "invalid landmark identifier"
    );
    Ok(())
}
fn component(reference: &ComponentReference) -> Result<()> {
    valid_text(&reference.peer_id)?;
    valid_text(&reference.component_id)?;
    valid_text(&reference.manifest_hash)
}
fn recognition(value: &Recognition) -> Result<()> {
    valid_text(&value.landmark_type)?;
    valid_text(&value.identity)?;
    Ok(())
}
pub(crate) fn provenance(value: &Provenance) -> Result<()> {
    for reference in [&value.detected_by, &value.mapped_by].into_iter().flatten() {
        component(reference)?;
    }
    ensure!(
        value.detected_by.is_none() || value.source_observation.is_some(),
        "detected landmark lacks source observation"
    );
    for source in [&value.source_observation, &value.camera_observation]
        .into_iter()
        .flatten()
    {
        valid_text(&source.output.peer_id)?;
        valid_text(&source.output.component_id)?;
        valid_text(&source.output.component_manifest_hash)?;
        valid_text(&source.output.output_id)?;
        valid_text(&source.output.slot)?;
        valid_text(&source.output.manifest_hash)?;
        auki_components::clock::validate_clock(&source.clock).map_err(anyhow::Error::msg)?;
    }
    if let (Some(detector), Some(source)) = (&value.detected_by, &value.source_observation) {
        ensure!(
            detector.peer_id == source.output.peer_id
                && detector.component_id == source.output.component_id
                && detector.manifest_hash == source.output.component_manifest_hash,
            "landmark detector differs from observation producer"
        );
    }
    Ok(())
}

impl Landmarks {
    pub fn empty(frame: impl Into<String>) -> Self {
        Self {
            schema: SCHEMA.into(),
            frame: frame.into(),
            anchors: BTreeMap::new(),
            landmarks: BTreeMap::new(),
            components: vec![],
        }
    }

    pub fn validate(&self, frame: &str) -> Result<()> {
        ensure!(
            self.schema == SCHEMA && self.frame == frame,
            "landmark schema/frame mismatch"
        );
        valid_text(frame)?;
        ensure!(
            self.anchors.len() <= 4096
                && self.landmarks.len() <= 16384
                && self.components.len() <= 4096,
            "landmark registry exceeds limits"
        );
        let mut local_frames = BTreeSet::new();
        for (id, anchor) in &self.anchors {
            valid_text(id)?;
            ensure!(
                id == &anchor.id && !self.landmarks.contains_key(id),
                "duplicate or mismatched anchor identity"
            );
            recognition(&anchor.recognition)?;
            provenance(&anchor.provenance)?;
            let pose = &anchor.pose;
            valid_text(&pose.from_frame_id)?;
            ensure!(
                pose.to_frame_id == frame
                    && pose.from_frame_id != frame
                    && local_frames.insert(&pose.from_frame_id),
                "invalid or ambiguous anchor frame"
            );
            crate::local_mapper::from_rigid(pose).validate()?;
        }
        for (id, landmark) in &self.landmarks {
            valid_text(id)?;
            ensure!(
                id == &landmark.id && landmark.position.iter().all(|v| v.is_finite()),
                "invalid landmark identity/position"
            );
            recognition(&landmark.recognition)?;
            provenance(&landmark.provenance)?;
            let expected = match &landmark.parent_anchor {
                Some(parent) => {
                    &self
                        .anchors
                        .get(parent)
                        .context("landmark parent is absent")?
                        .pose
                        .from_frame_id
                }
                None => &self.frame,
            };
            ensure!(
                &landmark.frame == expected,
                "landmark position frame differs from parent"
            );
        }
        for capability in &self.components {
            component(&capability.component)?;
            if let Some(types) = &capability.landmark_types {
                let mut seen = BTreeSet::new();
                for kind in types {
                    valid_text(kind)?;
                    ensure!(seen.insert(kind), "duplicate landmark capability");
                }
            }
        }
        Ok(())
    }

    pub fn by_type<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Landmark> {
        self.landmarks
            .values()
            .filter(move |point| point.recognition.landmark_type == kind)
    }

    /// Resolve a single position to map frame for consumers such as PnP. No
    /// orientation is required or manufactured on a landmark.
    pub fn position_in_map(&self, id: &str) -> Result<[f64; 3]> {
        self.validate(&self.frame)?;
        let landmark = self.landmarks.get(id).context("unknown landmark")?;
        if let Some(parent) = &landmark.parent_anchor {
            let anchor = &self.anchors[parent];
            let point = auki_odometry::Pose {
                translation: landmark.position,
                ..crate::local_mapper::identity()
            };
            Ok(crate::local_mapper::compose(
                &crate::local_mapper::from_rigid(&anchor.pose),
                &point,
            )?
            .translation)
        } else {
            Ok(landmark.position)
        }
    }

    /// Convert map-space poses and standalone points while preserving child
    /// coordinates in the physical marker's unchanged local axes.
    pub fn in_frame(&self, target: &str) -> Result<Self> {
        self.validate(&self.frame)?;
        let from = crate::conventions::registry("home", &self.frame)?;
        let to = crate::conventions::registry("home", target)?;
        let mut result = self.clone();
        result.frame = target.into();
        for anchor in result.anchors.values_mut() {
            let pose = crate::conventions::pose(
                &crate::local_mapper::from_rigid(&anchor.pose),
                &from,
                &to,
            )?;
            anchor.pose = crate::local_mapper::rigid(&pose, &anchor.pose.from_frame_id, target);
        }
        for landmark in result
            .landmarks
            .values_mut()
            .filter(|p| p.parent_anchor.is_none())
        {
            landmark.position = crate::conventions::point(landmark.position, &from, &to)?;
            landmark.frame = target.into();
        }
        result.validate(target)?;
        Ok(result)
    }
}

/// The default Portal mapper's policy, deliberately separate from generic node
/// validation. Only recognized Auki Portals are projected (not synthetic QRs).
pub fn portal_map(
    portals: &MapSnapshot,
    mapped_by: Option<ComponentReference>,
    evidence: &BTreeMap<String, Provenance>,
) -> Result<Landmarks> {
    portals.validate()?;
    let mut result = Landmarks::empty(&portals.scenegraph.map.frame.id);
    if let Some(component) = &mapped_by {
        result.components.push(ComponentCapabilities {
            component: component.clone(),
            landmark_types: Some(vec![PORTAL.into()]),
        });
    }
    for portal in portals.scenegraph.anchors.values() {
        // Configured/imported SDK anchors use bare shortcodes; camera evidence
        // uses the decoded URL. This does not broaden camera QR admission.
        let identity = crate::local_mapper::portal_code(&portal.payload).or_else(|| {
            (portal.payload.len() == 11
                && portal
                    .payload
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
            .then_some(portal.payload.as_str())
        });
        let Some(identity) = identity else {
            continue;
        };
        let mut origin = evidence.get(&portal.anchor_id).cloned().unwrap_or_default();
        if origin.mapped_by.is_none() {
            origin.mapped_by = mapped_by.clone();
        }
        let mut pose = portal.pose_in_map.clone();
        // Reference overlays can share the SDK's source frame with observed
        // anchors. Give each scene node its own explicitly equivalent frame.
        pose.from_frame_id = format!("landmark-anchor:{}", portal.anchor_id);
        let anchor = Anchor {
            id: portal.anchor_id.clone(),
            recognition: Recognition {
                landmark_type: PORTAL.into(),
                identity: identity.into(),
            },
            pose,
            provenance: origin.clone(),
        };
        let half = portal.side_length_m / 2.;
        for (feature, position) in CORNERS.into_iter().zip([
            [-half, half, 0.],
            [half, half, 0.],
            [half, -half, 0.],
            [-half, -half, 0.],
        ]) {
            let id = format!("{}/{}", anchor.id, feature);
            result.landmarks.insert(
                id.clone(),
                Landmark {
                    id,
                    recognition: Recognition {
                        identity: format!("{identity}/{feature}"),
                        ..anchor.recognition.clone()
                    },
                    position,
                    frame: anchor.pose.from_frame_id.clone(),
                    parent_anchor: Some(anchor.id.clone()),
                    provenance: origin.clone(),
                },
            );
        }
        result.anchors.insert(anchor.id.clone(), anchor);
    }
    result.validate(&result.frame)?;
    Ok(result)
}

pub fn from_registries(registries: &Value, frame: &str) -> Result<Landmarks> {
    let Some(value) = registries.get("landmarks") else {
        return Ok(Landmarks::empty(frame));
    };
    let result: Landmarks = serde_json::from_value(value.clone())?;
    result.validate(frame)?;
    Ok(result)
}

pub fn portal_evidence(metadata: &Value) -> Result<BTreeMap<String, Provenance>> {
    let mut result = BTreeMap::new();
    if let Some(observations) = metadata["local_mapping"]["observed_portals"].as_object() {
        for (id, observation) in observations {
            if let Some(value) = observation.get("provenance") {
                result.insert(id.clone(), serde_json::from_value(value.clone())?);
            }
        }
    }
    Ok(result)
}
