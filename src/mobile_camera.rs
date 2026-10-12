//! Phone RGB camera on the connection's existing SDK Catalog/Component endpoint.
use anyhow::{Result, ensure};
use auki_components::*;
use std::sync::{Arc, Mutex};

pub const PRODUCT: &str = "phone-camera";
pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 480;
pub const MAX_JPEG: usize = 1024 * 1024;

pub const METADATA_SCHEMA: &str = "spatial.phone-camera/v1";

/// What the phone knows about one frame beyond its pixels: which AR session
/// took it and the pinhole intrinsics of the image as sent.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub session: String,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    /// How far the image was turned clockwise from the camera's own
    /// orientation before sending, so a person sees it upright: 0, 90, 180
    /// or 270. The intrinsics describe the image as sent.
    pub rotation_deg: u16,
}
impl Geometry {
    /// A quarter turn sends the image on its side: 480 wide, 640 tall.
    pub fn size(&self) -> (u32, u32) {
        if matches!(self.rotation_deg, 90 | 270) { (HEIGHT, WIDTH) } else { (WIDTH, HEIGHT) }
    }
    /// The camera's own frame when the image is not turned. A turned image
    /// has its own frame, named after the camera frame it was turned from.
    pub fn frame_id(&self) -> String {
        match self.rotation_deg {
            0 => format!("phone-camera:{}", self.session),
            turn => format!("phone-camera:{}:turned-{turn}", self.session),
        }
    }
    fn valid(&self) -> bool {
        let (width, height) = self.size();
        uuid::Uuid::parse_str(&self.session).is_ok()
            && matches!(self.rotation_deg, 0 | 90 | 180 | 270)
            && [self.fx, self.fy, self.cx, self.cy].iter().all(|v| v.is_finite())
            && self.fx > 0.
            && self.fy > 0.
            && (0. ..=width as f64).contains(&self.cx)
            && (0. ..=height as f64).contains(&self.cy)
    }
}

fn payload((width, height): (u32, u32)) -> PayloadContract {
    PayloadContract::Camera(CameraPayloadContract {
        datatype: VideoFrame::DATATYPE.into(),
        schema: "auki.video-frame/v1".into(),
        encoding: "jpeg".into(),
        width,
        height,
        nominal_frame_rate_hz: Some(10),
        observes: "visible_light".into(),
    })
}

/// Catalog metadata says honestly what a reader can do with the image. No
/// pose Product is named: Home's camera overlay expects a robot's odometry.
fn metadata(geometry: Option<&Geometry>) -> serde_json::Value {
    match geometry {
        Some(g) => serde_json::json!({
            "calibration": {
                "K": [g.fx, 0., g.cx, 0., g.fy, g.cy, 0., 0., 1.],
                "D": [0., 0., 0., 0., 0.],
                "distortion_model": "plumb_bob",
                "width": g.size().0,
                "height": g.size().1,
            },
            "calibration_source": "AR session camera intrinsics, for the image as sent",
            "image_geometry": if g.rotation_deg == 0 { "ar_camera_image" } else { "ar_camera_image_turned_upright" },
            "camera_frame": format!("phone-camera:{}", g.session),
            "rotation_from_camera_frame_deg": g.rotation_deg,
            "timestamp_basis": "AR capture timestamp; device-local; a new AR session is a new clock",
        }),
        None => serde_json::json!({
            "calibration_status": "unavailable",
            "image_geometry": "unspecified",
            "timestamp_basis": "capture timestamp; device-local",
        }),
    }
}

fn capture(
    runtime: &ComponentRuntime,
    output: &ConfiguredObservable<VideoFrame>,
    geometry: Arc<Mutex<Option<Geometry>>>,
) -> Result<BufferProductCapture<VideoFrame>> {
    Ok(runtime.capture_buffer_with_metadata(
        PRODUCT,
        output,
        BufferLimits {
            max_entries: Some(1),
            max_bytes: Some(MAX_JPEG),
            target_duration: None,
        },
        |frame: &VideoFrame| frame.bytes.len(),
        move |observation| {
            let geometry = geometry.lock().map_err(|_| "camera geometry unavailable")?;
            Ok(Some(CatalogProductMetadata {
                schema: METADATA_SCHEMA.into(),
                source_sequence: observation.sequence,
                value: metadata(geometry.as_ref()),
            }))
        },
    )?)
}

pub struct Camera {
    endpoint: Arc<auki_component_protocol::ComponentProtocolEndpoint>,
    output: ConfiguredObservable<VideoFrame>,
    capture: Option<BufferProductCapture<VideoFrame>>,
    geometry: Arc<Mutex<Option<Geometry>>>,
    placed: Option<(String, u16)>,
    size: (u32, u32),
    runtime: ComponentRuntime,
    component: Component,
    last_timestamp: u64,
}
impl Camera {
    pub fn new(products: &crate::mobile_products::MobileProducts) -> Result<Self> {
        let component =
            products
                .runtime
                .component(ComponentSpec::new("phone-rgb-camera").observable(
                    ObservableContract {
                        name: "frames".into(),
                        datatype: VideoFrame::DATATYPE.into(),
                        schema: "auki.video-frame/v1".into(),
                        access: vec![ObservationAccess::FollowNew],
                        exposure: Exposure::Cluster,
                    },
                ))?;
        let session = uuid::Uuid::new_v4().to_string();
        let owner = component.manifest().peer_id.clone();
        let clock = crate::model::clock_entry(&owner, &session);
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "frames",
                session.clone(),
                crate::model::clock_ref(&clock),
                payload((WIDTH, HEIGHT)),
            )
            .in_registered_frame(auki_registry::FrameRegistryEntry::in_convention(
                &owner,
                format!("phone-rgb:{session}"),
                auki_registry::CoordinateConvention::OpenGl,
            )),
        )?;
        component.expose()?;
        let geometry = Arc::new(Mutex::new(None));
        let capture = capture(&products.runtime, &output, geometry.clone())?;
        products.endpoint.export_product(&capture.product())?;
        Ok(Self {
            endpoint: products.endpoint.clone(),
            output,
            capture: Some(capture),
            geometry,
            placed: None,
            size: (WIDTH, HEIGHT),
            runtime: products.runtime.clone(),
            component,
            last_timestamp: 0,
        })
    }
    pub fn demanded(&self) -> bool {
        self.endpoint.product_active_subscriptions(PRODUCT) > 0
    }
    /// A frame with its AR session and intrinsics. The first frame of a
    /// session moves the Output into that session's camera frame and capture
    /// clock, the ones phone depth and phone pose declare.
    pub fn publish_calibrated(
        &mut self,
        timestamp: u64,
        jpeg: Vec<u8>,
        geometry: Geometry,
    ) -> Result<()> {
        ensure!(geometry.valid(), "Invalid camera geometry");
        self.place(&geometry.session, geometry.rotation_deg, geometry.size(), geometry.frame_id(), timestamp)?;
        *self.geometry()? = Some(geometry);
        self.publish(timestamp, jpeg)
    }
    fn geometry(&self) -> Result<std::sync::MutexGuard<'_, Option<Geometry>>> {
        self.geometry
            .lock()
            .map_err(|_| anyhow::anyhow!("Camera geometry unavailable"))
    }
    /// Declare which AR session the camera is in as soon as it is known, so
    /// the Catalog relates it to depth and pose before any frame is asked for.
    /// A camera already placed in that session keeps how its image is turned.
    pub fn enter_session(&mut self, session: &str, timestamp: u64) -> Result<()> {
        if self.placed.as_ref().is_some_and(|(placed, _)| placed == session) {
            return Ok(());
        }
        self.place(session, 0, (WIDTH, HEIGHT), format!("phone-camera:{session}"), timestamp)
    }
    /// A new AR session, or the image being turned, is a new Output: its
    /// frame, clock or size changed.
    fn place(&mut self, session: &str, turn: u16, size: (u32, u32), frame_id: String, timestamp: u64) -> Result<()> {
        if self.placed.as_ref().is_some_and(|(placed, turned)| placed == session && *turned == turn) {
            return Ok(());
        }
        uuid::Uuid::parse_str(session)?;
        let owner = self.runtime.peer_id().to_string();
        let mut clock = crate::model::clock_entry(&owner, session);
        clock.clock_id = "arkit-capture".into();
        let replacement = self
            .component
            .replace_configured_observable(
                &self.output,
                ConfiguredObservableSpec::new(
                    "frames",
                    format!("frames:{session}:{}", uuid::Uuid::new_v4()),
                    crate::model::clock_ref(&clock),
                    payload(size),
                )
                .in_registered_frame(auki_registry::FrameRegistryEntry::opengl(&owner, frame_id)),
                timestamp,
            )?
            .replacement;
        if let Some(old) = self.capture.take() {
            self.endpoint.unexport_product(PRODUCT);
            old.delete()?;
        }
        self.output = replacement;
        self.placed = Some((session.into(), turn));
        self.size = size;
        self.last_timestamp = 0;
        *self.geometry()? = None;
        let capture = capture(&self.runtime, &self.output, self.geometry.clone())?;
        self.endpoint.export_product(&capture.product())?;
        self.capture = Some(capture);
        Ok(())
    }
    pub fn publish(&mut self, timestamp: u64, jpeg: Vec<u8>) -> Result<()> {
        ensure!(
            timestamp > self.last_timestamp,
            "Camera timestamp did not advance"
        );
        ensure!(
            !jpeg.is_empty() && jpeg.len() <= MAX_JPEG,
            "Invalid camera frame size"
        );
        let dimensions =
            image::ImageReader::with_format(std::io::Cursor::new(&jpeg), image::ImageFormat::Jpeg)
                .into_dimensions()?;
        ensure!(dimensions == self.size, "Invalid camera dimensions");
        let (width, height) = self.size;
        self.output.publish(
            timestamp,
            Arc::new(VideoFrame {
                width,
                height,
                encoding: "jpeg".into(),
                bytes: jpeg.into(),
            }),
        )?;
        self.last_timestamp = timestamp;
        ensure!(
            self.capture.as_ref().is_some_and(|c| c.errors().is_empty()),
            "Camera retention failed"
        );
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
