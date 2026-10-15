//! Shared retention policy for consumed SDK Products. Capture timestamps are
//! never rewritten to UTC: time transforms are a separate relationship.
use auki_components::BufferLimits;
use std::time::Duration;

/// Finite reads (for example opening a scene snapshot) use the same SDK
/// retention rules as streams. This stores only Products Home actually reads.
#[derive(Default)]
pub struct Fetched(
    std::collections::BTreeMap<String, (std::time::Instant, Box<dyn std::any::Any + Send + Sync>)>,
);
impl Fetched {
    pub fn retain<T>(
        &mut self,
        response: &auki_component_protocol::RemoteObservations<T>,
    ) -> anyhow::Result<()>
    where
        T: auki_components::ContractType + serde::Serialize + Send + Sync + 'static,
    {
        use anyhow::ensure;
        use auki_components::{Envelope, RetainedProduct};
        ensure!(
            response.product.producer == response.producer.reference(),
            "Fetched Product provenance mismatch"
        );
        let key = serde_json::to_string(&response.product.reference())?;
        self.0.retain(|_, (used, _)| used.elapsed() < WINDOW);
        if !self.0.contains_key(&key) {
            ensure!(self.0.len() < 32, "Consumed snapshot history limit reached");
            let retained = RetainedProduct::<T>::imported_buffer(
                response.product.clone(),
                response.product_manifest_hash.clone(),
                response.producer.clone(),
                structured(),
                |v: &T| serde_json::to_vec(v).map_or(usize::MAX, |v| v.len()),
            )?;
            self.0
                .insert(key.clone(), (std::time::Instant::now(), Box::new(retained)));
        }
        let (used, retained) = self.0.get_mut(&key).unwrap();
        *used = std::time::Instant::now();
        let retained = retained
            .downcast_ref::<RetainedProduct<T>>()
            .ok_or_else(|| anyhow::anyhow!("Fetched Product datatype changed"))?;
        for o in &response.observations {
            ensure!(
                o.output == response.producer.reference(),
                "Fetched observation provenance mismatch"
            );
            if retained
                .latest_existing()?
                .is_some_and(|last| last.sequence >= o.sequence)
            {
                continue;
            }
            retained
                .buffer()
                .append_shared(std::sync::Arc::new(Envelope::new(
                    o.sequence,
                    o.timestamp_ns,
                    o.clone(),
                )))?;
        }
        Ok(())
    }
}

/// Binary camera snapshots outlive their one-shot subscription. These are
/// imported SDK Products, never image JSON, and do not keep cameras active.
#[derive(Default)]
pub(crate) struct Cameras(
    std::collections::BTreeMap<String, CameraHistory>,
);
struct CameraHistory {
    used: std::time::Instant,
    snapshot: Option<(std::time::Instant, auki_components::RetainedProduct<auki_components::VideoFrame>)>,
}
impl Cameras {
    pub fn expire(&mut self) {
        self.0.retain(|_, history| history.used.elapsed() < WINDOW);
        for history in self.0.values_mut() {
            if history.snapshot.as_ref().is_some_and(|(used, _)| used.elapsed() >= WINDOW) {
                history.snapshot = None;
            }
        }
    }
    /// Reserve before opening a one-shot stream, so concurrent requests are
    /// bounded even while waiting for their first observation.
    pub fn reserve(&mut self, reference: &auki_components::ProductReference) -> anyhow::Result<()> {
        self.expire();
        let key = serde_json::to_string(reference)?;
        anyhow::ensure!(self.0.contains_key(&key) || self.0.len() < 8,
            "Home camera snapshot history limit reached (8 cameras)");
        self.0.entry(key).or_insert(CameraHistory { used: std::time::Instant::now(), snapshot: None }).used = std::time::Instant::now();
        Ok(())
    }
    pub fn retain(&mut self, source: &auki_components::RetainedProduct<auki_components::VideoFrame>) -> anyhow::Result<serde_json::Value> {
        use auki_components::{RetainedProduct, VideoFrame};
        self.reserve(&source.reference())?;
        let key = serde_json::to_string(&source.reference())?;
        let history = self.0.get_mut(&key).unwrap();
        if history.snapshot.is_none() {
            history.snapshot = Some((std::time::Instant::now(), RetainedProduct::imported_buffer(
                source.manifest.clone(), source.manifest_hash.clone(), source.producer.clone(),
                cameras(), |frame: &VideoFrame| frame.bytes.len(),
            )?));
        }
        let (used, retained) = history.snapshot.as_mut().unwrap();
        *used = std::time::Instant::now();
        anyhow::ensure!(retained.producer == source.producer, "Camera history producer changed");
        for record in source.buffer().snapshot(0, u64::MAX) {
            anyhow::ensure!(record.payload.output == source.producer.reference(),
                "Camera history observation provenance mismatch");
            if retained.buffer().range().last_sequence.is_some_and(|last| last >= record.payload.sequence) {
                continue;
            }
            // Share the original envelope, including Output, sequence, capture
            // timestamp and binary bytes. No restamping or payload conversion.
            retained.buffer().append_shared(record)?;
        }
        Ok(coverage(retained.buffer().snapshot(0, u64::MAX).iter().map(|record| record.payload.timestamp_ns)))
    }
}

pub const WINDOW: Duration = Duration::from_secs(10);

pub fn poses() -> BufferLimits {
    BufferLimits {
        max_entries: Some(4096),
        max_bytes: Some(4 * 1024 * 1024),
        target_duration: Some(WINDOW),
    }
}
pub fn cameras() -> BufferLimits {
    BufferLimits {
        max_entries: Some(1200),
        max_bytes: Some(128 * 1024 * 1024),
        target_duration: Some(WINDOW),
    }
}
pub fn depth() -> BufferLimits {
    BufferLimits {
        max_entries: Some(600),
        max_bytes: Some(64 * 1024 * 1024),
        target_duration: Some(WINDOW),
    }
}
pub fn structured() -> BufferLimits {
    BufferLimits {
        max_entries: Some(4096),
        max_bytes: Some(16 * 1024 * 1024),
        target_duration: Some(WINDOW),
    }
}

/// Report what is actually retained; hard byte/entry bounds can shorten history.
pub fn coverage(timestamps: impl Iterator<Item = u64>) -> serde_json::Value {
    let times = timestamps.collect::<Vec<_>>();
    let duration = times
        .first()
        .zip(times.last())
        .map(|(first, last)| last.saturating_sub(*first))
        .unwrap_or(0);
    serde_json::json!({"target_ms":WINDOW.as_millis() as u64,"retained_ms":duration/1_000_000,
        "entries":times.len(),"first_timestamp_ns":times.first().map(u64::to_string),
        "last_timestamp_ns":times.last().map(u64::to_string),"full_window":duration>=WINDOW.as_nanos() as u64})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camera_snapshots_keep_original_binary_envelopes_and_expire() -> anyhow::Result<()> {
        use auki_components::*;
        let runtime = ComponentRuntime::new("peer");
        let clock = auki_time::SessionClock::new("peer", "session", "capture");
        let component = runtime.component(ComponentSpec::new("camera").observable(ObservableContract {
            name: "frames".into(), datatype: VideoFrame::DATATYPE.into(),
            schema: crate::camera_frame::SCHEMA.into(), access: vec![ObservationAccess::FollowNew],
            exposure: Exposure::Cluster,
        }))?;
        let output = component.configured_observable(ConfiguredObservableSpec::new(
            "frames", "camera", crate::model::clock_ref(&clock.registry_entry()),
            PayloadContract::Camera(CameraPayloadContract {
                datatype: VideoFrame::DATATYPE.into(), schema: crate::camera_frame::SCHEMA.into(),
                width: 2, height: 2, encoding: "rgb8".into(), nominal_frame_rate_hz: Some(1),
                observes: "visible_light".into(),
            }),
        ).in_registered_frame(auki_registry::FrameRegistryEntry::opengl("peer", "camera")))?;
        component.expose()?;
        let source = runtime.capture_buffer("camera", &output, cameras(), |frame: &VideoFrame| frame.bytes.len())?;
        let mut cache = Cameras::default();
        let base = 9_007_199_254_740_993_u64;
        for second in 0..=12 {
            output.publish(base + second * 1_000_000_000, std::sync::Arc::new(VideoFrame {
                width: 2, height: 2, encoding: "rgb8".into(), bytes: vec![second as u8; 12].into(),
            }))?;
            cache.retain(&source.product())?;
        }
        let original = source.product().buffer().snapshot(0, u64::MAX);
        let key = serde_json::to_string(&source.product().reference())?;
        let retained = &cache.0[&key].snapshot.as_ref().unwrap().1;
        assert_eq!(retained.reference(), source.product().reference());
        assert_eq!(retained.producer.clock, crate::model::clock_ref(&clock.registry_entry()));
        assert_eq!(retained.producer, source.product().producer);
        let copies = retained.buffer().snapshot(0, u64::MAX);
        assert_eq!(copies.len(), 11);
        assert_eq!(retained.buffer().range().retained_payload_bytes, 11 * 12);
        assert_eq!(copies[0].payload.timestamp_ns, base + 2_000_000_000);
        for (original, copy) in original.iter().zip(&copies) {
            assert!(std::sync::Arc::ptr_eq(original, copy));
            assert_eq!(copy.sequence, copy.payload.sequence);
            assert_eq!(copy.timestamp_ns, copy.payload.timestamp_ns);
            assert_eq!(copy.payload.output, output.manifest().reference());
        }
        // Repeated reads merge by source sequence, never create new samples.
        cache.retain(&source.product())?;
        assert_eq!(cache.0[&key].snapshot.as_ref().unwrap().1.buffer().range().entries, 11);
        // An open PiP keeps a budget reservation, not an old one-shot's bytes.
        cache.0.get_mut(&key).unwrap().snapshot.as_mut().unwrap().0 = std::time::Instant::now() - WINDOW;
        cache.reserve(&source.product().reference())?;
        assert!(cache.0[&key].snapshot.is_none());
        cache.0.get_mut(&key).unwrap().used = std::time::Instant::now() - WINDOW;
        cache.expire();
        assert!(cache.0.is_empty());
        for n in 0..8 {
            let mut reference = source.product().reference(); reference.product_id = format!("camera-{n}");
            cache.reserve(&reference)?;
        }
        assert!(cache.reserve(&source.product().reference()).is_err());
        Ok(())
    }
    #[test]
    fn every_policy_has_ten_seconds_and_hard_limits() {
        for policy in [poses(), cameras(), depth(), structured()] {
            assert_eq!(policy.target_duration, Some(WINDOW));
            assert!(policy.max_entries.unwrap() > 0);
            assert!(policy.max_bytes.unwrap() > 0);
        }
    }
    #[test]
    fn sdk_retains_capture_window_without_rewriting_nanoseconds() {
        use auki_components::*;
        let runtime = ComponentRuntime::new("peer");
        let clock = auki_time::SessionClock::new("peer", "session", "capture");
        let component = runtime
            .component(ComponentSpec::new("test").observable(ObservableContract {
                name: "value".into(),
                datatype: crate::capture_time::SCHEMA.into(),
                schema: crate::capture_time::SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }))
            .unwrap();
        let output = component
            .configured_observable(ConfiguredObservableSpec::new(
                "value",
                "test",
                crate::model::clock_ref(&clock.registry_entry()),
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "test".into(),
                    datatype: crate::capture_time::SCHEMA.into(),
                    schema: crate::capture_time::SCHEMA.into(),
                    observes: "test".into(),
                    unit: None,
                }),
            ))
            .unwrap();
        component.expose().unwrap();
        let capture = runtime
            .capture_buffer(
                "test",
                &output,
                structured(),
                |_: &crate::capture_time::Sample| 16,
            )
            .unwrap();
        let base = 9_007_199_254_740_993_u64;
        for t in 0..=20 {
            output
                .publish(
                    base + t * 1_000_000_000,
                    std::sync::Arc::new(crate::capture_time::Sample {
                        offset_ns: 0,
                        uncertainty_ns: 0,
                    }),
                )
                .unwrap();
        }
        let retained = capture.product().buffer().snapshot(0, u64::MAX);
        assert_eq!(
            retained.first().unwrap().payload.timestamp_ns,
            base + 10_000_000_000
        );
        assert_eq!(
            retained.last().unwrap().payload.timestamp_ns,
            base + 20_000_000_000
        );
        assert_eq!(retained.len(), 11);
        assert_eq!(
            coverage(retained.iter().map(|o| o.payload.timestamp_ns))["retained_ms"],
            10_000
        );
    }
}
