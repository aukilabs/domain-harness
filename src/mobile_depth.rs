//! Phone-local LiDAR measurements plus explicit alignment evidence. Only Home
//! composes the transforms and feeds its existing raw-observation voxel mapper.
use crate::local_mapper::{canonical_origin, compose, from_rigid, inverse, rigid};
use anyhow::{Result, anyhow, ensure};
use auki_components::clock::ClockRegistryEntry;
use auki_components::*;
use auki_scenegraph::{MapSnapshot, RigidTransform, component::SnapshotReference};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const COMPONENT: &str = "home-phone-depth";
pub const SCHEMA: &str = "spatial.phone-depth/v1";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alignment {
    pub timestamp_ns: u64,
    pub portal_id: String,
    pub portal_to_session: RigidTransform,
    pub session_to_home: RigidTransform,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub session: String,
    pub timestamp_ns: u64,
    pub camera_to_session: RigidTransform,
    pub alignment: Alignment,
    pub points: Vec<[f64; 3]>,
    /// Optional sRGB camera color per point, sampled from the same frame.
    /// Omitted when absent, so colorless captures are unchanged on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<Vec<[u8; 3]>>,
}
impl ContractType for Sample {
    const DATATYPE: &'static str = SCHEMA;
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub sequence: u64,
    pub sample: Sample,
    pub source: ProductReference,
    pub clock: ClockRegistryEntry,
    pub sensor_frame: auki_registry::FrameRegistryEntry,
}
impl ContractType for Envelope {
    const DATATYPE: &'static str = "spatial.phone-depth-envelope/v1";
}
fn valid_pose(p: &RigidTransform) -> bool {
    p.translation
        .iter()
        .chain(p.rotation_wxyz.iter())
        .all(|v| v.is_finite())
        && (p.rotation_wxyz.iter().map(|v| v * v).sum::<f64>() - 1.).abs() < 0.001
}
pub fn validate(peer: &str, e: &Envelope, canonical: &MapSnapshot) -> Result<()> {
    let s = &e.sample;
    uuid::Uuid::parse_str(&s.session)?;
    let camera = format!("phone-camera:{}", s.session);
    let session = format!("phone-ar:{}", s.session);
    ensure!(
        e.source.peer_id == peer
            && e.source.product_id == format!("phone-depth:{}", s.session)
            && !e.source.manifest_hash.is_empty(),
        "Depth source identity mismatch"
    );
    let mut expected_clock = crate::model::clock_entry(peer, &s.session);
    expected_clock.clock_id = "arkit-capture".into();
    ensure!(e.clock == expected_clock, "Depth capture clock mismatch");
    ensure!(
        e.sensor_frame == auki_registry::FrameRegistryEntry::opengl(peer, &camera),
        "Depth camera convention mismatch"
    );
    ensure!(
        s.timestamp_ns >= s.alignment.timestamp_ns && s.alignment.timestamp_ns > 0,
        "Depth predates calibration"
    );
    ensure!(
        s.colors.as_ref().is_none_or(|c| c.len() == s.points.len()),
        "Depth color/point count differs"
    );
    ensure!(
        s.camera_to_session.from_frame_id == camera
            && s.camera_to_session.to_frame_id == session
            && s.alignment.portal_to_session.to_frame_id == session
            && s.alignment.session_to_home.from_frame_id == session
            && s.alignment.session_to_home.to_frame_id == canonical.scenegraph.map.frame.id,
        "Depth transform endpoints mismatch"
    );
    let portal = canonical
        .scenegraph
        .anchors
        .get(&s.alignment.portal_id)
        .ok_or_else(|| anyhow!("Unknown depth calibration Portal"))?;
    ensure!(
        s.alignment.portal_to_session.from_frame_id == portal.pose_in_map.from_frame_id,
        "Depth Portal frame mismatch"
    );
    ensure!(
        valid_pose(&s.camera_to_session)
            && valid_pose(&s.alignment.portal_to_session)
            && valid_pose(&s.alignment.session_to_home),
        "Invalid depth transform"
    );
    let expected = compose(
        &from_rigid(&portal.pose_in_map),
        &inverse(&from_rigid(&s.alignment.portal_to_session))?,
    )?;
    let supplied = from_rigid(&s.alignment.session_to_home);
    let delta = compose(&inverse(&expected)?, &supplied)?;
    ensure!(
        delta.translation.iter().map(|v| v * v).sum::<f64>() < 0.0001
            && delta.rotation_xyzw[3].abs() > 0.9999,
        "Depth alignment disagrees with Portal observation"
    );
    ensure!(
        !s.points.is_empty()
            && s.points.len() <= 2048
            && s.points.iter().all(|p| p.iter().all(|v| v.is_finite())
                && p[2] <= -0.2
                && p[2] >= -5.
                && p.iter().map(|v| v * v).sum::<f64>() <= 64.),
        "Invalid or oversized phone depth frame"
    );
    Ok(())
}

#[derive(Default)]
pub struct Inbox {
    pending: BTreeMap<String, (Envelope, Instant)>,
    watermarks: BTreeMap<(String, String), (u64, u64)>,
}
pub type SharedInbox = Arc<Mutex<Inbox>>;
impl Inbox {
    pub fn accept(
        &mut self,
        peer: &str,
        envelope: Envelope,
        canonical: &MapSnapshot,
    ) -> Result<bool> {
        validate(peer, &envelope, canonical)?;
        let key = (peer.to_owned(), envelope.source.manifest_hash.clone());
        if self.watermarks.get(&key).is_some_and(|(sequence, t)| {
            *sequence >= envelope.sequence || *t >= envelope.sample.timestamp_ns
        }) {
            return Ok(false);
        }
        ensure!(
            self.watermarks.contains_key(&key) || self.watermarks.len() < 128,
            "Depth source capacity reached"
        );
        self.watermarks
            .insert(key, (envelope.sequence, envelope.sample.timestamp_ns));
        self.pending.insert(peer.into(), (envelope, Instant::now()));
        Ok(true)
    }
    pub fn take(&mut self) -> Vec<Envelope> {
        std::mem::take(&mut self.pending)
            .into_values()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(2))
            .map(|(e, _)| e)
            .collect()
    }
}
pub fn localize(
    e: Envelope,
    shared: &MapSnapshot,
    reference: SnapshotReference,
    shared_frame: &str,
) -> Result<crate::collaborative_mapper::LocalizedScan> {
    let s = e.sample;
    let portal = shared
        .scenegraph
        .anchors
        .get(&s.alignment.portal_id)
        .ok_or_else(|| anyhow!("Unknown shared depth Portal"))?;
    let clock = crate::model::clock_ref(&e.clock);
    let origin = canonical_origin(
        s.alignment.timestamp_ns,
        clock.clone(),
        &from_rigid(&s.alignment.portal_to_session),
        portal,
    )?;
    let camera_in_shared = compose(&origin.odometry_to_local, &from_rigid(&s.camera_to_session))?;
    // Preserve every measured endpoint and its color. Home independently bounds
    // free-space ray tracing; that budget must not thin the observed surface.
    let colors = (0..s.points.len()).map(|i| s.colors.as_ref().map(|c| c[i])).collect();
    let observation = auki_voxel_map::DepthObservation {
        source: e.source.clone(),
        sequence: e.sequence,
        timestamp_ns: s.timestamp_ns,
        clock: clock.clone(),
        sensor_frame_id: e.sensor_frame.frame_id.clone(),
        hit_points_m: s.points,
        pose: auki_voxel_map::TimedSensorPose {
            timestamp_ns: s.timestamp_ns,
            clock: clock.clone(),
            sensor_to_map: rigid(&camera_in_shared, &e.sensor_frame.frame_id, shared_frame),
            portal_snapshot: reference,
        },
    };
    Ok(crate::collaborative_mapper::LocalizedScan {
        observation,
        binding: auki_voxel_map::SensorBinding {
            sensor_product: e.source,
            sensor_frame: e.sensor_frame,
            observation_clock: clock,
        },
        colors,
        origin,
    })
}

/// Actual SDK Product identity accompanies the bounded pushed observation.
/// The same raw phone-local samples are also available as an exported Product.
pub struct Publisher {
    pub session: String,
    output: ConfiguredObservable<Sample>,
    capture: Option<BufferProductCapture<Sample>>,
    endpoint: Arc<auki_component_protocol::ComponentProtocolEndpoint>,
    clock: ClockRegistryEntry,
    sensor_frame: auki_registry::FrameRegistryEntry,
    _component: Component,
    _runtime: ComponentRuntime,
}
impl Publisher {
    pub fn new(products: &crate::mobile_products::MobileProducts, session: &str) -> Result<Self> {
        uuid::Uuid::parse_str(session)?;
        let runtime = products.runtime.clone();
        let owner = runtime.peer_id().to_string();
        let mut clock = crate::model::clock_entry(&owner, session);
        clock.clock_id = "arkit-capture".into();
        let sensor_frame =
            auki_registry::FrameRegistryEntry::opengl(&owner, format!("phone-camera:{session}"));
        let component = runtime.component(ComponentSpec::new("phone-depth").observable(
            ObservableContract {
                name: "depth".into(),
                datatype: SCHEMA.into(),
                schema: SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            },
        ))?;
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "depth",
                format!("depth:{session}:{}", uuid::Uuid::new_v4()),
                crate::model::clock_ref(&clock),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "depth".into(),
                    datatype: SCHEMA.into(),
                    schema: SCHEMA.into(),
                    observes: "phone_local_lidar".into(),
                    unit: Some("meters".into()),
                }),
            )
            .in_registered_frame(sensor_frame.clone()),
        )?;
        component.expose()?;
        let capture = runtime.capture_buffer(
            format!("phone-depth:{session}"),
            &output,
            BufferLimits {
                max_entries: Some(2),
                max_bytes: Some(1024 * 1024),
                target_duration: None,
            },
            |s: &Sample| serde_json::to_vec(s).map_or(usize::MAX, |v| v.len()),
        )?;
        let endpoint = products.endpoint.clone();
        endpoint.export_product(&capture.product())?;
        Ok(Self {
            session: session.into(),
            output,
            capture: Some(capture),
            endpoint,
            clock,
            sensor_frame,
            _component: component,
            _runtime: runtime,
        })
    }
    pub fn publish(&self, sample: Sample) -> Result<Envelope> {
        ensure!(sample.session == self.session, "Depth session changed");
        let capture = self
            .capture
            .as_ref()
            .ok_or_else(|| anyhow!("Depth capture unavailable"))?;
        let (observation, _) = self
            .output
            .publish(sample.timestamp_ns, Arc::new(sample.clone()))?;
        ensure!(capture.errors().is_empty(), "Depth Product capture failed");
        Ok(Envelope {
            sequence: observation.sequence,
            sample,
            source: capture.product().reference(),
            clock: self.clock.clone(),
            sensor_frame: self.sensor_frame.clone(),
        })
    }
    /// Reconfigure the stable Component on an AR reset, without remounting protocols.
    pub fn reconfigure(&mut self, session: &str, timestamp_ns: u64) -> Result<()> {
        uuid::Uuid::parse_str(session)?;
        let owner = self._runtime.peer_id().to_string();
        let mut clock = crate::model::clock_entry(&owner, session);
        clock.clock_id = "arkit-capture".into();
        let frame =
            auki_registry::FrameRegistryEntry::opengl(&owner, format!("phone-camera:{session}"));
        let replacement = self
            ._component
            .replace_configured_observable(
                &self.output,
                ConfiguredObservableSpec::new(
                    "depth",
                    format!("depth:{session}:{}", uuid::Uuid::new_v4()),
                    crate::model::clock_ref(&clock),
                    PayloadContract::Structured(StructuredPayloadContract {
                        modality: "depth".into(),
                        datatype: SCHEMA.into(),
                        schema: SCHEMA.into(),
                        observes: "phone_local_lidar".into(),
                        unit: Some("meters".into()),
                    }),
                )
                .in_registered_frame(frame.clone()),
                timestamp_ns,
            )?
            .replacement;
        if let Some(old) = self.capture.take() {
            self.endpoint
                .unexport_product(&old.product().manifest.product_id);
            old.delete()?;
        }
        self.output = replacement;
        self.session = session.into();
        self.clock = clock;
        self.sensor_frame = frame;
        let capture = self._runtime.capture_buffer(
            format!("phone-depth:{session}"),
            &self.output,
            BufferLimits {
                max_entries: Some(2),
                max_bytes: Some(1024 * 1024),
                target_duration: None,
            },
            |s: &Sample| serde_json::to_vec(s).map_or(usize::MAX, |v| v.len()),
        )?;
        self.endpoint.export_product(&capture.product())?;
        self.capture = Some(capture);
        Ok(())
    }
    pub async fn close(mut self) -> Result<()> {
        if let Some(capture) = self.capture.take() {
            self.endpoint
                .unexport_product(&capture.product().manifest.product_id);
            capture.delete()?;
        }
        Ok(())
    }
}
pub async fn send(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
    envelope: &Envelope,
) -> Result<bool> {
    use auki_component_protocol::{CatalogResponse, ComponentProtocolClient};
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
            .ok_or_else(|| anyhow!("Home depth receiver unavailable"))?;
        let response = client
            .invoke_exact::<_, bool>(
                expected,
                route,
                component.manifest.reference(),
                "ingest",
                "phone-depth",
                uuid::Uuid::new_v4().to_string(),
                Some(Duration::from_secs(1)),
                envelope,
            )
            .await?;
        return Ok(response.result);
    }
    anyhow::bail!("Home depth receiver unavailable")
}
