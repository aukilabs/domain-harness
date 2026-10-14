//! What a peer measured, declared so any other peer can place it without being
//! told how in every message.
//!
//! Two Products, alongside the peer's camera pose (`mobile_odometry`):
//!
//! - **A transform** from the peer's AR session frame into a scene frame that
//!   another peer owns, from the Portal it aligned on. It is published under
//!   the same profile as every other transform (`compact_pose`), so there is
//!   one contract to read whether the transform is a pose or an alignment.
//! - **Depth points**: LiDAR points in the camera frame at a capture time. No
//!   pose and no alignment inside.
//!
//! A reader places a depth capture by following frames: points (camera frame)
//! → the transform out of the camera frame at the capture time → the transform
//! out of the AR session frame. It finds each by its contract's `from_frame`,
//! never by a Product's name.
use crate::compact_pose::{Contract as TransformContract, Update as Transform};
#[cfg(test)]
use crate::compact_pose::FrameRef;
use anyhow::{Result, anyhow, ensure};
use auki_components::*;
use auki_odometry::{Pose, Tracking};
use auki_protocols::registry::v3::{RegistryEntryEnvelope, RegistryKind};
use auki_registry::FrameRegistryEntry;
use auki_scenegraph::RigidTransform;
use serde::Deserialize;
use std::sync::Arc;

pub use crate::depth_points::{Points, PointsContract, POINTS_SCHEMA, POINTS_METADATA, MAX_POINTS};

/// Named after the frame it starts from, which is unique to the AR session.
pub fn transform_product_id(session: &str) -> String {
    format!("frame-transform:phone-ar:{session}")
}
pub fn points_product_id(session: &str) -> String {
    format!("depth-points:{session}")
}

/// Where the app's AR session sits in the scene, as the app last measured it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub session: String,
    pub timestamp_ns: u64,
    pub session_to_scene: RigidTransform,
}

/// The transform as a pose: translation, and an XYZW quaternion of unit length.
pub fn pose(transform: &RigidTransform) -> Result<Pose> {
    let [w, x, y, z] = transform.rotation_wxyz;
    let norm = (w * w + x * x + y * y + z * z).sqrt();
    ensure!(norm.is_finite() && norm > 0.5, "Invalid alignment rotation");
    let pose = Pose { translation: transform.translation, rotation_xyzw: [x / norm, y / norm, z / norm, w / norm] };
    pose.validate()?;
    Ok(pose)
}

fn structured(modality: &str, schema: &str, observes: &str, unit: Option<&str>) -> PayloadContract {
    PayloadContract::Structured(StructuredPayloadContract {
        modality: modality.into(),
        datatype: schema.into(),
        schema: schema.into(),
        observes: observes.into(),
        unit: unit.map(Into::into),
    })
}
fn observable(name: &str, schema: &str) -> ObservableContract {
    ObservableContract {
        name: name.into(),
        datatype: schema.into(),
        schema: schema.into(),
        access: vec![ObservationAccess::FollowNew],
        exposure: Exposure::Cluster,
    }
}

pub struct Publisher {
    session: String,
    home_peer: String,
    home_frame: String,
    alignment: ConfiguredObservable<Transform>,
    alignment_capture: BufferProductCapture<Transform>,
    points: ConfiguredObservable<Points>,
    points_capture: BufferProductCapture<Points>,
    endpoint: Arc<auki_component_protocol::ComponentProtocolEndpoint>,
    aligned_at: u64,
    lost: bool,
    captured_at: u64,
    _components: [Component; 2],
}
impl Publisher {
    pub fn new(products: &crate::mobile_products::MobileProducts, session: &str, home_peer: &str, home_frame: &str) -> Result<Self> {
        uuid::Uuid::parse_str(session)?;
        let runtime = products.runtime.clone();
        let owner = runtime.peer_id().to_string();
        let mut clock = crate::model::clock_entry(&owner, session);
        clock.clock_id = "arkit-capture".into();
        let clock_ref = crate::model::clock_ref(&clock);
        let camera = FrameRegistryEntry::opengl(&owner, format!("phone-camera:{session}"));
        let ar = FrameRegistryEntry::opengl(&owner, format!("phone-ar:{session}"));
        let scene = crate::conventions::registry(home_peer, home_frame)?;
        {
            // The phone serves the definitions it owns. The scene frame is Home's to serve.
            let mut entries = products.definitions.lock().map_err(|_| anyhow!("Registry unavailable"))?;
            for entry in [
                RegistryEntryEnvelope { kind: RegistryKind::Clock, id: clock.clock_id.clone(), hash: clock.hash(), canonical_json: String::from_utf8(clock.canonical_bytes())? },
                RegistryEntryEnvelope { kind: RegistryKind::Frame, id: camera.frame_id.clone(), hash: camera.hash(), canonical_json: String::from_utf8(camera.canonical_bytes())? },
                RegistryEntryEnvelope { kind: RegistryKind::Frame, id: ar.frame_id.clone(), hash: ar.hash(), canonical_json: String::from_utf8(ar.canonical_bytes())? },
            ] {
                if !entries.iter().any(|e| e.kind == entry.kind && e.id == entry.id && e.hash == entry.hash) {
                    entries.push(entry);
                }
            }
        }
        let endpoint = products.endpoint.clone();

        // Where this AR session sits in the scene: a transform like any other, whose
        // destination is a frame the scene's owner defines.
        // It holds after the Portal leaves view, until refined or lost.
        let transform_contract = TransformContract { from_frame: (&ar).into(), to_frame: (&scene).into(), clock: clock_ref.clone(),
            session_id: session.into(), lifetime: crate::compact_pose::Lifetime::Held };
        let aligner = runtime.component(ComponentSpec::new("frame-transforms").observable(observable("transform", crate::compact_pose::SCHEMA)))?;
        let alignment: ConfiguredObservable<Transform> = aligner.configured_observable(
            ConfiguredObservableSpec::new("transform", transform_contract.output_id(), clock_ref.clone(),
                structured("pose", crate::compact_pose::SCHEMA, "ar_session_in_scene", None))
            .in_registered_frame(scene),
        )?;
        transform_contract.validate_output(&alignment.manifest())?;
        aligner.expose()?;
        let described = transform_contract.clone();
        let alignment_capture = runtime.capture_buffer_with_metadata(
            transform_product_id(session), &alignment,
            BufferLimits { max_entries: Some(8), max_bytes: Some(64 * 1024), target_duration: None },
            |t: &Transform| serde_json::to_vec(t).map_or(usize::MAX, |v| v.len()),
            move |observation| Ok(Some(CatalogProductMetadata {
                schema: crate::compact_pose::METADATA.into(),
                source_sequence: observation.sequence,
                value: serde_json::to_value(&described).map_err(|e| e.to_string())?,
            })),
        )?;
        endpoint.export_product(&alignment_capture.product())?;

        let points_contract = PointsContract {
            frame: (&camera).into(), clock: clock_ref.clone(), session_id: session.into(),
        };
        let depth = runtime.component(ComponentSpec::new("depth-points").observable(observable("points", POINTS_SCHEMA)))?;
        let points: ConfiguredObservable<Points> = depth.configured_observable(
            ConfiguredObservableSpec::new("points", points_contract.output_id(), clock_ref,
                structured("depth", POINTS_SCHEMA, "lidar_points_in_camera_frame", Some("meters")))
            .in_registered_frame(camera),
        )?;
        depth.expose()?;
        let described = points_contract.clone();
        let points_capture = runtime.capture_buffer_with_metadata(
            points_product_id(session), &points,
            BufferLimits { max_entries: Some(4), max_bytes: Some(2 * 1024 * 1024), target_duration: None },
            |p: &Points| serde_json::to_vec(p).map_or(usize::MAX, |v| v.len()),
            move |observation| Ok(Some(CatalogProductMetadata {
                schema: POINTS_METADATA.into(),
                source_sequence: observation.sequence,
                value: serde_json::to_value(&described).map_err(|e| e.to_string())?,
            })),
        )?;
        endpoint.export_product(&points_capture.product())?;

        Ok(Self {
            session: session.into(), home_peer: home_peer.into(), home_frame: home_frame.into(),
            alignment, alignment_capture, points, points_capture, endpoint,
            aligned_at: 0, lost: false, captured_at: 0, _components: [aligner, depth],
        })
    }
    /// A new AR session, or another scene, is another pair of frames: new Outputs.
    pub fn serves(&self, session: &str, home_peer: &str, home_frame: &str) -> bool {
        self.session == session && self.home_peer == home_peer && self.home_frame == home_frame
    }
    /// The transform measured at `timestamp_ns`. One already published is not published again.
    pub fn align(&mut self, timestamp_ns: u64, session_to_scene: &RigidTransform) -> Result<()> {
        ensure!(
            session_to_scene.from_frame_id == format!("phone-ar:{}", self.session) && session_to_scene.to_frame_id == self.home_frame,
            "Transform endpoints mismatch"
        );
        if timestamp_ns <= self.aligned_at && !self.lost {
            return Ok(());
        }
        let update = Transform { tracking: Tracking::Valid { pose: pose(session_to_scene)? } };
        update.validate()?;
        let at = timestamp_ns.max(self.aligned_at + 1);
        self.alignment.publish(at, Arc::new(update))?;
        ensure!(self.alignment_capture.errors().is_empty(), "Transform Product capture failed");
        self.aligned_at = at;
        self.lost = false;
        Ok(())
    }
    /// The peer no longer knows where it is in the scene. Said once.
    pub fn lose(&mut self) -> Result<()> {
        if self.lost || self.aligned_at == 0 {
            return Ok(());
        }
        self.alignment.publish(self.aligned_at + 1, Arc::new(Transform { tracking: Tracking::Lost }))?;
        self.aligned_at += 1;
        self.lost = true;
        Ok(())
    }
    /// One depth capture. A capture that does not advance the clock is skipped.
    pub fn capture(&mut self, timestamp_ns: u64, points: &[[f64; 3]], colors: Option<&[[u8; 3]]>) -> Result<()> {
        if timestamp_ns <= self.captured_at {
            return Ok(());
        }
        let update = Points { points: points.to_vec(), colors: colors.map(<[_]>::to_vec) };
        update.validate()?;
        self.points.publish(timestamp_ns, Arc::new(update))?;
        ensure!(self.points_capture.errors().is_empty(), "Depth points Product capture failed");
        self.captured_at = timestamp_ns;
        Ok(())
    }
    pub fn close(self) -> Result<()> {
        self.endpoint.unexport_product(&self.alignment_capture.product().manifest.product_id);
        self.endpoint.unexport_product(&self.points_capture.product().manifest.product_id);
        self.alignment_capture.delete()?;
        self.points_capture.delete()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn points_carry_only_what_changes() {
        let points = Points { points: vec![[0., 0., -1.]], colors: None };
        points.validate().unwrap();
        assert_eq!(serde_json::to_string(&points).unwrap(), r#"{"points":[[0.0,0.0,-1.0]]}"#);
        assert!(serde_json::from_str::<Points>(r#"{"points":[[0,0,-1]],"pose":{}}"#).is_err());
    }
    #[test]
    fn bad_measurements_are_refused() {
        assert!(Points { points: vec![], colors: None }.validate().is_err());
        assert!(Points { points: vec![[f64::NAN, 0., 0.]], colors: None }.validate().is_err());
        assert!(Points { points: vec![[0., 0., -1.]], colors: Some(vec![]) }.validate().is_err());
        let skewed = RigidTransform { from_frame_id: "a".into(), to_frame_id: "b".into(), translation: [0.; 3], rotation_wxyz: [0.; 4] };
        assert!(pose(&skewed).is_err(), "a rotation must be a unit quaternion");
    }
    #[test]
    fn a_changed_reference_is_a_different_output() {
        let frame = |id: &str| FrameRef { peer_id: "p".into(), id: id.into(), hash: "a".into() };
        let clock = ClockReference { peer_id: "p".into(), id: "clock".into(), hash: "c".into() };
        let mut points = PointsContract { frame: frame("camera"), clock, session_id: "s".into() };
        let before = points.output_id();
        points.frame.hash = "b".into();
        assert_ne!(before, points.output_id());
    }
}
