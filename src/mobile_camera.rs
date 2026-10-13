//! Phone RGB camera on the connection's existing SDK Catalog/Component endpoint.
use anyhow::{Result, ensure};
use auki_components::*;
use std::sync::{Arc, Mutex};

pub const PRODUCT: &str = "phone-camera";
pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 480;
pub const MAX_JPEG: usize = 1024 * 1024;
pub const MAX_LONG_SIDE: u32 = 1280;
/// Frames each of the camera and depth image Products keeps.
pub const RETAINED_FRAMES: usize = 16;
pub const DEPTH_IMAGE_PRODUCT: &str = "phone-depth-image";

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
    /// Capture time of the Portal alignment the phone was using when it took
    /// this frame, or 0 if it was not aligned. A reader that sees it change
    /// knows poses before and after are not in the same alignment.
    pub alignment_timestamp_ns: u64,
}
/// The sizes a calibrated frame may have: the camera's 4:3 image, 640 to 1280
/// on its long side, on its side exactly when it was turned a quarter.
pub fn size_allowed((width, height): (u32, u32), rotation_deg: u16) -> bool {
    let (long, short) = (width.max(height), width.min(height));
    long * 3 == short * 4
        && (WIDTH..=MAX_LONG_SIDE).contains(&long)
        && (height > width) == matches!(rotation_deg, 90 | 270)
}
impl Geometry {
    /// The camera's own frame when the image is not turned. A turned image
    /// has its own frame, named after the camera frame it was turned from.
    pub fn frame_id(&self) -> String {
        match self.rotation_deg {
            0 => format!("phone-camera:{}", self.session),
            turn => format!("phone-camera:{}:turned-{turn}", self.session),
        }
    }
    fn valid(&self, (width, height): (u32, u32)) -> bool {
        uuid::Uuid::parse_str(&self.session).is_ok()
            && matches!(self.rotation_deg, 0 | 90 | 180 | 270)
            && size_allowed((width, height), self.rotation_deg)
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

/// Catalog metadata says honestly what a reader can do with the image. The
/// pose Product is named under its own key, not the one Home's camera overlay
/// reads: that overlay expects a robot's odometry and alignment.
fn metadata(geometry: Option<&(Geometry, (u32, u32))>) -> serde_json::Value {
    match geometry {
        Some((g, size)) => serde_json::json!({
            "calibration": {
                "K": [g.fx, 0., g.cx, 0., g.fy, g.cy, 0., 0., 1.],
                "D": [0., 0., 0., 0., 0.],
                "distortion_model": "plumb_bob",
                "width": size.0,
                "height": size.1,
            },
            "calibration_source": "AR session camera intrinsics, for the image as sent",
            "image_geometry": if g.rotation_deg == 0 { "ar_camera_image" } else { "ar_camera_image_turned_upright" },
            "camera_frame": format!("phone-camera:{}", g.session),
            "rotation_from_camera_frame_deg": g.rotation_deg,
            // While aligned, the pose Product holds a pose with exactly this
            // frame's capture timestamp, on the same clock.
            "capture_pose_product_id": (g.alignment_timestamp_ns > 0).then(|| crate::mobile_odometry::product_id(&g.session)),
            "alignment_timestamp_ns": (g.alignment_timestamp_ns > 0).then_some(g.alignment_timestamp_ns),
            // The LiDAR depth image of the same capture has the same timestamp.
            "depth_image_product_id": DEPTH_IMAGE_PRODUCT,
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
    geometry: Arc<Mutex<Option<(Geometry, (u32, u32))>>>,
) -> Result<BufferProductCapture<VideoFrame>> {
    Ok(runtime.capture_buffer_with_metadata(
        PRODUCT,
        output,
        BufferLimits {
            // About three seconds of frames, so a reader that is a moment
            // behind still finds the picture for a depth image or pose.
            max_entries: Some(RETAINED_FRAMES),
            max_bytes: Some(8 * 1024 * 1024),
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
pub fn registry_calibration(network:&serde_json::Value,peer:&str,id:&str,camera_frame:&str)->Result<serde_json::Value>{
 let row=network["peers"].as_array().and_then(|peers|peers.iter().find(|p|p["peer_id"]==peer)).ok_or_else(||anyhow::anyhow!("Camera peer Catalog unavailable"))?;
 let catalog:CatalogSnapshot=serde_json::from_value(row["catalog"].clone())?;
 let product=catalog.products.iter().find(|p|p.manifest.peer_id==peer&&p.manifest.product_id==format!("phone-camera-calibration:{id}")).ok_or_else(||anyhow::anyhow!("Referenced camera configuration unavailable in peer Catalog; refresh or rebuild phone"))?;
 let metadata=product.metadata.as_ref().ok_or_else(||anyhow::anyhow!("Camera configuration metadata missing"))?;
 metadata.validate().map_err(anyhow::Error::msg)?;
 ensure!(metadata.schema=="spatial.phone-camera-calibration/v1"&&metadata.value["configuration_id"]==id&&metadata.value["camera_frame_id"]==camera_frame,"Camera calibration reference mismatch");
 ensure!(catalog.components.iter().flat_map(|c|c.current_outputs.values()).any(|o|o.manifest.peer_id==peer&&o.manifest.reference()==product.manifest.producer&&o.manifest.spatial_frame.as_ref().is_some_and(|f|f.definition==auki_registry::FrameRegistryEntry::opengl(peer,camera_frame))&&matches!(&o.manifest.payload,PayloadContract::Structured(c) if c.datatype==CalibrationReference::DATATYPE&&c.schema==CalibrationReference::DATATYPE)),"Camera calibration producer/frame contract mismatch");
 crate::ar_preview::camera_model(&metadata.value["calibration"])?;
 Ok(metadata.value["calibration"].clone())
}
#[derive(Clone,serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {pub session:String,pub camera:crate::mobile_pose::CameraIntrinsics}
#[derive(Clone)]
pub struct RegisteredConfiguration {pub id:String,pub configuration:Configuration}
#[derive(Clone,serde::Serialize,serde::Deserialize)]
pub struct CalibrationReference {pub configuration_id:String}
impl ContractType for CalibrationReference {const DATATYPE:&'static str="spatial.camera-calibration-reference/v1";}
/// Intrinsics live in Catalog metadata; the retained observation is only a reference.
pub struct CalibrationPublisher {capture:BufferProductCapture<CalibrationReference>,endpoint:Arc<auki_component_protocol::ComponentProtocolEndpoint>,product:String,_component:Component}
impl CalibrationPublisher {
 pub fn new(products:&crate::mobile_products::MobileProducts,config:&RegisteredConfiguration)->Result<Self>{
  config.configuration.camera.validate()?;uuid::Uuid::parse_str(&config.configuration.session)?;
  let component=products.runtime.component(ComponentSpec::new("phone-camera-calibration").observable(ObservableContract{name:"configuration".into(),datatype:CalibrationReference::DATATYPE.into(),schema:CalibrationReference::DATATYPE.into(),access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster}))?;
  let owner=component.manifest().peer_id.clone();let clock=crate::model::clock_entry(&owner,&config.configuration.session);
  let output=component.configured_observable(ConfiguredObservableSpec::new("configuration",config.id.clone(),crate::model::clock_ref(&clock),PayloadContract::Structured(StructuredPayloadContract{datatype:CalibrationReference::DATATYPE.into(),schema:CalibrationReference::DATATYPE.into(),modality:"calibration".into(),observes:"camera_configuration".into(),unit:None})).in_registered_frame(auki_registry::FrameRegistryEntry::opengl(&owner,format!("phone-camera:{}",config.configuration.session))))?;
  component.expose()?;
  let product=format!("phone-camera-calibration:{}",config.id);
  let metadata=serde_json::json!({"configuration_id":config.id,"camera_frame_id":format!("phone-camera:{}",config.configuration.session),"calibration":config.configuration.camera.calibration()});
  let capture=products.runtime.capture_buffer_with_metadata(&product,&output,BufferLimits{max_entries:Some(1),max_bytes:Some(4096),target_duration:None},|_:&CalibrationReference|256,move |o|Ok(Some(CatalogProductMetadata{schema:"spatial.phone-camera-calibration/v1".into(),source_sequence:o.sequence,value:metadata.clone()})))?;
  products.endpoint.export_product(&capture.product())?;
  output.publish(1,Arc::new(CalibrationReference{configuration_id:config.id.clone()}))?;
  Ok(Self{capture,endpoint:products.endpoint.clone(),product,_component:component})
 }
 pub fn close(self)->Result<()>{self.endpoint.unexport_product(&self.product);self.capture.delete()?;Ok(())}
}

pub struct Camera {
    endpoint: Arc<auki_component_protocol::ComponentProtocolEndpoint>,
    output: ConfiguredObservable<VideoFrame>,
    capture: Option<BufferProductCapture<VideoFrame>>,
    geometry: Arc<Mutex<Option<(Geometry, (u32, u32))>>>,
    placed: Option<(String, u16, (u32, u32))>,
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
        let size = dimensions(&jpeg)?;
        ensure!(geometry.valid(size), "Invalid camera geometry");
        self.place(&geometry.session, geometry.rotation_deg, size, geometry.frame_id(), timestamp)?;
        *self.geometry()? = Some((geometry, size));
        self.publish(timestamp, jpeg)
    }
    fn geometry(&self) -> Result<std::sync::MutexGuard<'_, Option<(Geometry, (u32, u32))>>> {
        self.geometry
            .lock()
            .map_err(|_| anyhow::anyhow!("Camera geometry unavailable"))
    }
    /// Declare which AR session the camera is in as soon as it is known, so
    /// the Catalog relates it to depth and pose before any frame is asked for.
    /// A camera already placed in that session keeps how its image is turned.
    pub fn enter_session(&mut self, session: &str, timestamp: u64) -> Result<()> {
        if self.placed.as_ref().is_some_and(|(placed, _, _)| placed == session) {
            return Ok(());
        }
        self.place(session, 0, (WIDTH, HEIGHT), format!("phone-camera:{session}"), timestamp)
    }
    /// A new AR session, or the image being turned or resized, is a new
    /// Output: its frame, clock or size changed.
    fn place(&mut self, session: &str, turn: u16, size: (u32, u32), frame_id: String, timestamp: u64) -> Result<()> {
        if self.placed.as_ref().is_some_and(|(placed, turned, sized)| placed == session && *turned == turn && *sized == size) {
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
        self.placed = Some((session.into(), turn, size));
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
        ensure!(dimensions(&jpeg)? == self.size, "Invalid camera dimensions");
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
fn dimensions(jpeg: &[u8]) -> Result<(u32, u32)> {
    Ok(
        image::ImageReader::with_format(std::io::Cursor::new(jpeg), image::ImageFormat::Jpeg)
            .into_dimensions()?,
    )
}
