//! The phone's LiDAR depth image as a declared SDK Product, one per camera
//! frame sent, with the same capture timestamp on the same clock.
use anyhow::{Result, ensure};
use auki_components::*;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const COMPONENT: &str = "phone-depth-image";
pub const PRODUCT: &str = crate::mobile_camera::DEPTH_IMAGE_PRODUCT;
pub const SCHEMA: &str = "spatial.phone-depth-image/v1";
pub const METADATA_SCHEMA: &str = "spatial.phone-depth-image-metadata/v1";
pub const MAX_SIDE: u32 = 1024;

/// One depth image as the sensor gave it: not turned, in the AR session's
/// camera frame, row-major from the top-left pixel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepthImage {
    pub session: String,
    pub timestamp_ns: u64,
    pub width: u32,
    pub height: u32,
    /// Base64 of `width * height` little-endian u16 depths in millimetres
    /// along the optical axis. 0 means no measurement.
    pub depth_mm: String,
    /// Base64 of `width * height` bytes: 0 low, 1 medium, 2 high confidence.
    pub confidence: String,
    /// Pinhole intrinsics for this image's pixels.
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    /// Capture time of the Portal alignment in use, or 0 if not aligned.
    pub alignment_timestamp_ns: u64,
}
impl ContractType for DepthImage {
    const DATATYPE: &'static str = SCHEMA;
}
impl DepthImage {
    pub fn new(
        session: &str,
        timestamp_ns: u64,
        (width, height): (u32, u32),
        depth_mm: &[u16],
        confidence: &[u8],
        [fx, fy, cx, cy]: [f64; 4],
        alignment_timestamp_ns: u64,
    ) -> Result<Self> {
        let pixels = width as usize * height as usize;
        ensure!(
            depth_mm.len() == pixels && confidence.len() == pixels,
            "Depth image size mismatch"
        );
        let bytes: Vec<u8> = depth_mm.iter().flat_map(|v| v.to_le_bytes()).collect();
        let engine = base64::engine::general_purpose::STANDARD;
        let image = Self {
            session: session.into(),
            timestamp_ns,
            width,
            height,
            depth_mm: engine.encode(bytes),
            confidence: engine.encode(confidence),
            fx,
            fy,
            cx,
            cy,
            alignment_timestamp_ns,
        };
        image.validate()?;
        Ok(image)
    }
    pub fn validate(&self) -> Result<()> {
        uuid::Uuid::parse_str(&self.session)?;
        ensure!(self.timestamp_ns > 0, "Depth image has no capture time");
        ensure!(
            (1..=MAX_SIDE).contains(&self.width) && (1..=MAX_SIDE).contains(&self.height),
            "Invalid depth image size"
        );
        let pixels = self.width as usize * self.height as usize;
        // Base64 lengths are checked without decoding every frame twice.
        ensure!(
            self.depth_mm.len() == (pixels * 2).div_ceil(3) * 4
                && self.confidence.len() == pixels.div_ceil(3) * 4,
            "Depth image size mismatch"
        );
        ensure!(
            [self.fx, self.fy, self.cx, self.cy].iter().all(|v| v.is_finite())
                && self.fx > 0.
                && self.fy > 0.
                && (0. ..=self.width as f64).contains(&self.cx)
                && (0. ..=self.height as f64).contains(&self.cy),
            "Invalid depth image intrinsics"
        );
        Ok(())
    }
}

fn spec(owner: &str, session: &str) -> ConfiguredObservableSpec {
    let mut clock = crate::model::clock_entry(owner, session);
    clock.clock_id = "arkit-capture".into();
    ConfiguredObservableSpec::new(
        "frames",
        format!("depth-image:{session}:{}", uuid::Uuid::new_v4()),
        crate::model::clock_ref(&clock),
        PayloadContract::Structured(StructuredPayloadContract {
            modality: "depth".into(),
            datatype: SCHEMA.into(),
            schema: SCHEMA.into(),
            observes: "phone_lidar_depth_image".into(),
            unit: Some("millimeters".into()),
        }),
    )
    .in_registered_frame(auki_registry::FrameRegistryEntry::opengl(
        owner,
        format!("phone-camera:{session}"),
    ))
}

fn capture(
    runtime: &ComponentRuntime,
    output: &ConfiguredObservable<DepthImage>,
) -> Result<BufferProductCapture<DepthImage>> {
    Ok(runtime.capture_buffer_with_metadata(
        PRODUCT,
        output,
        BufferLimits {
            // As many as the camera keeps, so each retained picture has its depth.
            max_entries: Some(crate::mobile_camera::RETAINED_FRAMES),
            max_bytes: Some(8 * 1024 * 1024),
            target_duration: None,
        },
        |image: &DepthImage| image.depth_mm.len() + image.confidence.len() + 512,
        |observation| {
            let image = &observation.payload;
            Ok(Some(CatalogProductMetadata {
                schema: METADATA_SCHEMA.into(),
                source_sequence: observation.sequence,
                value: serde_json::json!({
                    "width": image.width,
                    "height": image.height,
                    "depth_encoding": "base64 of little-endian u16 millimetres along the optical axis; 0 is no measurement",
                    "confidence_encoding": "base64 of u8: 0 low, 1 medium, 2 high",
                    "image_geometry": "ar_depth_image, not turned, row-major from the top-left pixel",
                    "camera_frame": format!("phone-camera:{}", image.session),
                    "camera_product_id": crate::mobile_camera::PRODUCT,
                    "pose_product_id": (image.alignment_timestamp_ns > 0).then(|| crate::mobile_odometry::product_id(&image.session)),
                    "alignment_timestamp_ns": (image.alignment_timestamp_ns > 0).then_some(image.alignment_timestamp_ns),
                    "timestamp_basis": "AR capture timestamp, equal to the camera frame of the same capture",
                }),
            }))
        },
    )?)
}

pub struct Publisher {
    session: String,
    output: ConfiguredObservable<DepthImage>,
    capture: Option<BufferProductCapture<DepthImage>>,
    endpoint: Arc<auki_component_protocol::ComponentProtocolEndpoint>,
    last_timestamp: u64,
    component: Component,
    runtime: ComponentRuntime,
}
impl Publisher {
    pub fn new(products: &crate::mobile_products::MobileProducts, session: &str) -> Result<Self> {
        uuid::Uuid::parse_str(session)?;
        let runtime = products.runtime.clone();
        let component =
            runtime.component(ComponentSpec::new(COMPONENT).observable(ObservableContract {
                name: "frames".into(),
                datatype: SCHEMA.into(),
                schema: SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }))?;
        let output = component.configured_observable(spec(runtime.peer_id(), session))?;
        component.expose()?;
        let capture = capture(&runtime, &output)?;
        products.endpoint.export_product(&capture.product())?;
        Ok(Self {
            session: session.into(),
            output,
            capture: Some(capture),
            endpoint: products.endpoint.clone(),
            last_timestamp: 0,
            component,
            runtime,
        })
    }
    pub fn demanded(&self) -> bool {
        self.endpoint.product_active_subscriptions(PRODUCT) > 0
    }
    /// A new AR session is a new Output on the same Component.
    pub fn enter_session(&mut self, session: &str, timestamp_ns: u64) -> Result<()> {
        if self.session == session {
            return Ok(());
        }
        uuid::Uuid::parse_str(session)?;
        let replacement = self
            .component
            .replace_configured_observable(
                &self.output,
                spec(self.runtime.peer_id(), session),
                timestamp_ns,
            )?
            .replacement;
        self.withdraw()?;
        self.output = replacement;
        self.session = session.into();
        self.last_timestamp = 0;
        let capture = capture(&self.runtime, &self.output)?;
        self.endpoint.export_product(&capture.product())?;
        self.capture = Some(capture);
        Ok(())
    }
    /// An image that does not advance the capture clock is skipped, not an error.
    pub fn publish(&mut self, image: DepthImage) -> Result<()> {
        image.validate()?;
        self.enter_session(&image.session, image.timestamp_ns)?;
        if image.timestamp_ns <= self.last_timestamp {
            return Ok(());
        }
        let timestamp_ns = image.timestamp_ns;
        self.output.publish(timestamp_ns, Arc::new(image))?;
        self.last_timestamp = timestamp_ns;
        ensure!(
            self.capture.as_ref().is_some_and(|c| c.errors().is_empty()),
            "Depth image Product capture failed"
        );
        Ok(())
    }
    fn withdraw(&mut self) -> Result<()> {
        if let Some(capture) = self.capture.take() {
            self.endpoint.unexport_product(PRODUCT);
            capture.delete()?;
        }
        Ok(())
    }
    pub fn close(mut self) -> Result<()> {
        self.withdraw()
    }
}
