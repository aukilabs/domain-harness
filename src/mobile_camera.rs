//! Phone RGB camera on the connection's existing SDK Catalog/Component endpoint.
use anyhow::{Result, ensure};
use auki_components::*;
use std::sync::Arc;

pub const PRODUCT: &str = "phone-camera";
pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 480;
pub const MAX_JPEG: usize = 1024 * 1024;

pub struct Camera {
    endpoint: Arc<auki_component_protocol::ComponentProtocolEndpoint>,
    output: ConfiguredObservable<VideoFrame>,
    capture: BufferProductCapture<VideoFrame>,
    _component: Component,
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
                PayloadContract::Camera(CameraPayloadContract {
                    datatype: VideoFrame::DATATYPE.into(),
                    schema: "auki.video-frame/v1".into(),
                    encoding: "jpeg".into(),
                    width: WIDTH,
                    height: HEIGHT,
                    nominal_frame_rate_hz: Some(10),
                    observes: "visible_light".into(),
                }),
            )
            .in_registered_frame(auki_registry::FrameRegistryEntry::in_convention(
                &owner,
                format!("phone-rgb:{session}"),
                auki_registry::CoordinateConvention::OpenGl,
            )),
        )?;
        component.expose()?;
        let capture = products.runtime.capture_buffer(
            PRODUCT,
            &output,
            BufferLimits {
                max_entries: Some(1),
                max_bytes: Some(MAX_JPEG),
                target_duration: None,
            },
            |frame: &VideoFrame| frame.bytes.len(),
        )?;
        products.endpoint.export_product(&capture.product())?;
        Ok(Self {
            endpoint: products.endpoint.clone(),
            output,
            capture,
            _component: component,
            last_timestamp: 0,
        })
    }
    pub fn demanded(&self) -> bool {
        self.endpoint.product_active_subscriptions(PRODUCT) > 0
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
        ensure!(dimensions == (WIDTH, HEIGHT), "Invalid camera dimensions");
        self.output.publish(
            timestamp,
            Arc::new(VideoFrame {
                width: WIDTH,
                height: HEIGHT,
                encoding: "jpeg".into(),
                bytes: jpeg.into(),
            }),
        )?;
        self.last_timestamp = timestamp;
        ensure!(self.capture.errors().is_empty(), "Camera retention failed");
        Ok(())
    }
    pub fn close(self) -> Result<()> {
        self.endpoint.unexport_product(PRODUCT);
        self.capture.delete()?;
        Ok(())
    }
}
