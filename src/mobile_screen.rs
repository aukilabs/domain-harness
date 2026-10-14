//! The phone app's own screen, as a camera Product Home can watch. It is
//! captured and sent only while someone is subscribed: Home opening it in its
//! picture-in-picture is the request, closing it ends it.
use anyhow::{Result, ensure};
use auki_components::*;
use std::sync::Arc;

pub const PRODUCT: &str = "phone-screen";
/// Height of the picture sent, in pixels: the app is portrait. Its width
/// keeps the screen's shape.
pub const HEIGHT: u32 = 640;
pub const MIN_WIDTH: u32 = 200;
pub const MAX_JPEG: usize = 512 * 1024;
pub const METADATA_SCHEMA: &str = "spatial.phone-screen/v1";

/// Screen pictures are already upright, so the presentation is declared, not
/// worked out from a pose: a screen has none.
fn metadata() -> serde_json::Value {
    serde_json::json!({
        "presentation": {"schema": "spatial.video-presentation/v1", "rotation_clockwise_deg": 0},
        "image_geometry": "app_screen",
        "timestamp_basis": "screen capture presentation time; device-local",
    })
}

fn payload((width, height): (u32, u32)) -> PayloadContract {
    PayloadContract::Camera(CameraPayloadContract {
        datatype: VideoFrame::DATATYPE.into(),
        schema: crate::camera_frame::SCHEMA.into(),
        encoding: "jpeg".into(),
        width,
        height,
        nominal_frame_rate_hz: Some(8),
        observes: "display".into(),
    })
}

pub fn size_allowed((width, height): (u32, u32)) -> bool {
    height == HEIGHT && (MIN_WIDTH..=HEIGHT).contains(&width)
}

pub struct Screen {
    endpoint: Arc<auki_component_protocol::ComponentProtocolEndpoint>,
    output: ConfiguredObservable<VideoFrame>,
    capture: Option<BufferProductCapture<VideoFrame>>,
    size: (u32, u32),
    last_timestamp: u64,
    _component: Component,
}
impl Screen {
    /// A screen of `size` pixels as sent. A screen of another size is a new Screen.
    pub fn new(products: &crate::mobile_products::MobileProducts, size: (u32, u32)) -> Result<Self> {
        ensure!(size_allowed(size), "Invalid screen size");
        let component = products.runtime.component(ComponentSpec::new("phone-app-screen").observable(
            ObservableContract {
                name: "frames".into(),
                datatype: VideoFrame::DATATYPE.into(),
                schema: crate::camera_frame::SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            },
        ))?;
        let session = uuid::Uuid::new_v4().to_string();
        let owner = component.manifest().peer_id.clone();
        let mut clock = crate::model::clock_entry(&owner, &session);
        clock.clock_id = "screen-capture".into();
        // The picture's own image frame. Nothing relates it to space.
        let output = component.configured_observable(
            ConfiguredObservableSpec::new("frames", session.clone(), crate::model::clock_ref(&clock), payload(size))
                .in_registered_frame(auki_registry::FrameRegistryEntry::opengl(&owner, format!("phone-screen:{session}"))),
        )?;
        component.expose()?;
        let capture = products.runtime.capture_buffer_with_metadata(
            PRODUCT,
            &output,
            BufferLimits { max_entries: Some(4), max_bytes: Some(4 * MAX_JPEG), target_duration: None },
            |frame: &VideoFrame| frame.bytes.len(),
            |observation| {
                Ok(Some(CatalogProductMetadata {
                    schema: METADATA_SCHEMA.into(),
                    source_sequence: observation.sequence,
                    value: metadata(),
                }))
            },
        )?;
        products.endpoint.export_product_with_encoder(&capture.product(), crate::camera_frame::encoder())?;
        Ok(Self {
            endpoint: products.endpoint.clone(),
            output,
            capture: Some(capture),
            size,
            last_timestamp: 0,
            _component: component,
        })
    }
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
    pub fn demanded(&self) -> bool {
        self.endpoint.product_active_subscriptions(PRODUCT) > 0
    }
    pub fn publish(&mut self, timestamp: u64, jpeg: Vec<u8>) -> Result<()> {
        ensure!(timestamp > self.last_timestamp, "Screen timestamp did not advance");
        ensure!(!jpeg.is_empty() && jpeg.len() <= MAX_JPEG, "Invalid screen frame size");
        let size = image::ImageReader::with_format(std::io::Cursor::new(&jpeg), image::ImageFormat::Jpeg)
            .into_dimensions()?;
        ensure!(size == self.size, "Invalid screen dimensions");
        self.output.publish(
            timestamp,
            Arc::new(VideoFrame { width: size.0, height: size.1, encoding: "jpeg".into(), bytes: jpeg.into() }),
        )?;
        self.last_timestamp = timestamp;
        ensure!(self.capture.as_ref().is_some_and(|c| c.errors().is_empty()), "Screen retention failed");
        Ok(())
    }
    pub fn close(mut self) -> Result<()> {
        self.endpoint.unexport_product(PRODUCT);
        if let Some(capture) = self.capture.take() {
            capture.delete()?;
        }
        Ok(())
    }
}
