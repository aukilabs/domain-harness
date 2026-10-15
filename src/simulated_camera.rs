//! Camera Product; per-Product SDK subscriptions drive local Godot capture.
use anyhow::{Result, ensure};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::*;
use auki_sdk::AukiPeer;
use std::sync::Arc;

pub const PRODUCT: &str = "simulator-camera";
pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 480;
pub const MAX_JPEG: usize = 1024 * 1024;

pub struct Camera {
    pub runtime: ComponentRuntime,
    pub endpoint: ComponentProtocolEndpoint,
    pub session_clock: auki_time::SessionClock,
    time_transform: crate::capture_time::Publisher,
    _registry: auki_protocols::registry::RegistryEndpoint,
    output: ConfiguredObservable<VideoFrame>,
    capture: BufferProductCapture<VideoFrame>,
    _component: Component,
    last_timestamp: u64,
}
impl Camera {
    pub fn mount(peer: &AukiPeer) -> Result<Self> {
        let owner = peer.peer_id().to_string();
        let runtime = ComponentRuntime::new(owner.clone());
        let component = runtime.component(
            ComponentSpec::new("simulator-rgb-camera").observable(ObservableContract {
                name: "frames".into(),
                datatype: VideoFrame::DATATYPE.into(),
                schema: crate::camera_frame::SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }),
        )?;
        let session = uuid::Uuid::new_v4().to_string();
        let session_clock = auki_time::SessionClock::new(&owner, &session, "session");
        let clock = session_clock.registry_entry();
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "frames",
                session.clone(),
                crate::model::clock_ref(&clock),
                PayloadContract::Camera(CameraPayloadContract {
                    datatype: VideoFrame::DATATYPE.into(),
                    schema: crate::camera_frame::SCHEMA.into(),
                    encoding: "jpeg".into(),
                    width: WIDTH,
                    height: HEIGHT,
                    nominal_frame_rate_hz: Some(10),
                    observes: "visible_light".into(),
                }),
            )
            .in_registered_frame(auki_registry::FrameRegistryEntry::in_convention(
                &owner,
                format!("simulator:{session}:camera"),
                auki_registry::CoordinateConvention::OpenGl,
            )),
        )?;
        component.expose()?;
        let capture = runtime.capture_buffer(
            PRODUCT,
            &output,
            BufferLimits {
                max_entries: Some(1),
                max_bytes: Some(MAX_JPEG),
                target_duration: None,
            },
            |frame: &VideoFrame| frame.bytes.len(),
        )?;
        let endpoint = ComponentProtocolEndpoint::mount(peer.protocols(), runtime.clone())?;
        endpoint.export_product_with_encoder(&capture.product(), crate::camera_frame::encoder())?;
        let mut time_transform = crate::capture_time::Publisher::mount(&runtime, &endpoint, clock.clone())?;
        time_transform.sample(&crate::capture_time::SessionUtcClock(session_clock.clone()))?;
        let definitions = crate::product_registry::Definitions::default();
        crate::product_registry::clock(&definitions, &clock)?;
        crate::product_registry::clock(&definitions, &time_transform.contract.to_clock)?;
        let registry = crate::product_registry::mount(peer, runtime.clone(), definitions)?;
        Ok(Self {
            runtime,
            endpoint,
            session_clock,
            time_transform,
            _registry: registry,
            output,
            capture,
            _component: component,
            last_timestamp: 0,
        })
    }
    pub fn tick_clock(&mut self) -> Result<()> {
        self.time_transform.sample(&crate::capture_time::SessionUtcClock(self.session_clock.clone()))?;
        Ok(())
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
            "Camera frame size invalid"
        );
        let dimensions =
            image::ImageReader::with_format(std::io::Cursor::new(&jpeg), image::ImageFormat::Jpeg)
                .into_dimensions()?;
        ensure!(dimensions == (WIDTH, HEIGHT), "Camera dimensions changed");
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
    pub async fn close(self) -> Result<()> {
        self.capture.cancel();
        let registry = self._registry.close().await;
        let endpoint = self.endpoint.close().await;
        registry.map_err(Into::into).and(endpoint.map_err(Into::into))
    }
}
