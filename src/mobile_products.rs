//! One SDK service owner per phone connection; publishers only export Products.
use anyhow::{Result, anyhow};
use auki_component_protocol::ComponentProtocolEndpoint;
use auki_components::ComponentRuntime;
use std::sync::Arc;
use std::sync::Mutex;
use auki_protocols::registry::{RegistryEndpoint,v3::*};

/// The app measures native clock readings between two SDK session readings.
/// A frame timestamp is converted from that measured relationship, never from
/// when its encoded bytes arrive at the bridge. Native app state stays native.
pub struct CaptureTiming {
    pub clock: auki_time::SessionClock,
    /// Local FFI token: rejects a bracket sampled across a client replacement.
    /// Registry identity remains the full SDK clock reference, not this token.
    pub generation: u64,
    native: Mutex<[crate::capture_time::NativeMapping; 2]>,
    first_native: Mutex<[Option<u64>; 2]>,
    reset: std::sync::atomic::AtomicBool,
    held: Mutex<std::collections::BTreeMap<(String, u64), u64>>,
}
impl CaptureTiming {
    pub fn clock_reference(&self) -> Result<auki_components::ClockReference> {
        if self.reset.load(std::sync::atomic::Ordering::Acquire) { return Err(crate::capture_time::NativeClockReset.into()); }
        Ok(crate::clock_sync::reference(&self.clock.registry_entry()))
    }
    pub fn new(peer: &str) -> Self {
        static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self { clock: auki_time::SessionClock::new(peer, uuid::Uuid::new_v4().to_string(), "session"),
            generation: GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            native: Mutex::new(Default::default()), first_native: Mutex::new(Default::default()),
            reset: std::sync::atomic::AtomicBool::new(false), held: Mutex::new(Default::default()) }
    }
    pub fn observe(&self, kind: usize, before: u64, native: u64, after: u64) -> Result<()> {
        let mut clocks = self.native.lock().map_err(|_| anyhow!("Native clock unavailable"))?;
        if self.reset.load(std::sync::atomic::Ordering::Acquire) { return Err(crate::capture_time::NativeClockReset.into()); }
        let result = clocks.get_mut(kind).ok_or_else(|| anyhow!("Unknown native capture clock"))?
            .observe_pair(before, native, after);
        if result.as_ref().err().is_some_and(|e| e.downcast_ref::<crate::capture_time::NativeClockReset>().is_some()) {
            self.reset.store(true, std::sync::atomic::Ordering::Release);
        }
        result?;
        self.first_native.lock().map_err(|_| anyhow!("Native clock unavailable"))?[kind].get_or_insert(native);
        Ok(())
    }
    pub fn capture(&self, kind: usize, native: u64) -> Result<u64> {
        if self.reset.load(std::sync::atomic::Ordering::Acquire) { return Err(crate::capture_time::NativeClockReset.into()); }
        self.native.lock().map_err(|_| anyhow!("Native clock unavailable"))?
            .get(kind).ok_or_else(|| anyhow!("Unknown native capture clock"))?.convert(native)
    }
    /// Convert a copy for Products and onboarding history. Private app tracking
    /// retains native evidence; published timestamps use the SDK session clock.
    pub fn pose(&self, mut sample: crate::mobile_pose::Sample) -> Result<crate::mobile_pose::Sample> {
        sample.timestamp_ns = self.capture(0, sample.timestamp_ns)?;
        Ok(sample)
    }
    /// Calibrations keep the capture time measured when established, even after
    /// the native conversion history expires. This is not a fresh measurement.
    pub fn held(&self, session: &str, native: u64) -> Result<u64> {
        if self.reset.load(std::sync::atomic::Ordering::Acquire) { return Err(crate::capture_time::NativeClockReset.into()); }
        if native == 0 { return Ok(0); }
        let key = (session.to_owned(), native);
        let mut held = self.held.lock().map_err(|_| anyhow!("Calibration clock unavailable"))?;
        if let Some(at) = held.get(&key) { return Ok(*at); }
        let at = self.capture(0, native)?;
        if held.len() >= 32 { held.pop_first(); }
        held.insert(key, at);
        Ok(at)
    }
    /// An explicitly current, valid app Placement can establish a held-state
    /// baseline when its original evidence predates this session's first native
    /// measurement. This is an assertion now, NOT a newly captured measurement;
    /// the app's original native evidence is retained unchanged. Never used by
    /// dynamic captures, expired history, gaps, or reset clocks.
    pub fn assert_current_held(&self, session: &str, native: u64) -> Result<u64> {
        match self.held(session, native) {
            Ok(at) => Ok(at),
            Err(error) => {
                if error.downcast_ref::<crate::capture_time::NativeClockReset>().is_some() { return Err(error); }
                let first = self.first_native.lock().map_err(|_| anyhow!("Native clock unavailable"))?[0];
                if !first.is_some_and(|first| native > 0 && native < first) { return Err(error); }
                let at = self.clock.now_ns();
                let mut held = self.held.lock().map_err(|_| anyhow!("Calibration clock unavailable"))?;
                if held.len() >= 32 { held.pop_first(); }
                held.insert((session.to_owned(), native), at);
                Ok(at)
            }
        }
    }
}

/// A retained derived snapshot may be advertised again after the native
/// conversion window expires. Remember the one source time already converted;
/// publication refresh does not make it a new measurement.
#[derive(Default)]
pub(crate) struct RetainedTimestamp(Option<(u64, String, u64, u64)>);
impl RetainedTimestamp {
    pub fn convert(&mut self, timing: &CaptureTiming, session: &str, native: u64) -> Result<u64> {
        if timing.reset.load(std::sync::atomic::Ordering::Acquire) { return Err(crate::capture_time::NativeClockReset.into()); }
        if let Some((generation, old_session, old_native, at)) = &self.0
            && *generation == timing.generation && old_session == session && *old_native == native { return Ok(*at); }
        let at = timing.capture(0, native)?;
        self.0 = Some((timing.generation, session.into(), native, at));
        Ok(at)
    }
}

pub struct MobileProducts {
    pub runtime: ComponentRuntime,
    pub endpoint: Arc<ComponentProtocolEndpoint>,
    registry: RegistryEndpoint,
    pub definitions: Arc<Mutex<Vec<RegistryEntryEnvelope>>>,
    pub timing: Arc<CaptureTiming>,
    time_transform: Mutex<crate::capture_time::Publisher>,
}
impl MobileProducts {
    pub fn registered_clock(&self) -> Result<crate::clock_sync::RegisteredClock> {
        self.timing.clock_reference()?;
        let publisher = self.time_transform.lock().map_err(|_| anyhow!("Clock publisher unavailable"))?;
        crate::clock_sync::RegisteredClock::local(&publisher)
    }
    pub fn mount(peer: &auki_sdk::AukiPeer) -> Result<Self> {
        let runtime = ComponentRuntime::new(peer.peer_id().to_string());
        let endpoint = Arc::new(ComponentProtocolEndpoint::mount(
            peer.protocols(),
            runtime.clone(),
        )?);
        let definitions=Arc::new(Mutex::new(Vec::<RegistryEntryEnvelope>::new()));
        let entries=definitions.clone();
        let registry=RegistryEndpoint::mount(peer.protocols(),move |_:&auki_sdk::AuthenticatedPeer,request:&RegistryRequest|{
            let Ok(entries)=entries.lock() else {return RegistryResponse::Error{reason:"Registry unavailable".into()}};
            match request {
                RegistryRequest::List{kind}=>RegistryResponse::List{entries:entries.iter().filter(|e|e.kind==*kind).map(|e|RegistryListEntry{id:e.id.clone(),hash:e.hash.clone()}).collect()},
                RegistryRequest::Get{kind,id,hash}=>RegistryResponse::Get{entry:entries.iter().find(|e|e.kind==*kind&&e.id==*id&&e.hash==*hash).cloned()},
            }
        })?;
        let timing = Arc::new(CaptureTiming::new(&peer.peer_id().to_string()));
        let mut time_transform = crate::capture_time::Publisher::mount(&runtime, &endpoint, timing.clock.registry_entry().clone())?;
        for definition in [&time_transform.contract.from_clock, &time_transform.contract.to_clock] {
            definitions.lock().map_err(|_| anyhow!("Registry unavailable"))?.push(RegistryEntryEnvelope {
                kind: RegistryKind::Clock, id: definition.clock_id.clone(), hash: definition.hash(),
                canonical_json: String::from_utf8(definition.canonical_bytes())?,
            });
        }
        time_transform.sample(&crate::capture_time::SessionUtcClock(timing.clock.clone()))?;
        Ok(Self { runtime, endpoint, registry, definitions, timing, time_transform: Mutex::new(time_transform) })
    }
    pub fn sample_time(&self) -> Result<()> {
        self.time_transform.lock().map_err(|_| anyhow!("Clock publisher unavailable"))?
            .sample(&crate::capture_time::SessionUtcClock(self.timing.clock.clone()))?;
        Ok(())
    }
    /// Publishers must be closed/dropped before the connection's service owner.
    pub async fn close(self) -> Result<()> {
        self.registry.close().await?;
        Arc::try_unwrap(self.endpoint)
            .map_err(|_| anyhow!("Phone publishers still hold the SDK endpoint"))?
            .close()
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_capture_uses_measured_native_offset_and_preserves_held_time() -> Result<()> {
        let timing = CaptureTiming::new("phone");
        // Native clock is 50 seconds ahead; no frame/receipt time participates
        // in the clock calibration. A delayed frame remains 150 ms, not 200 ms.
        timing.observe(0, 100_000_000, 50_100_000_010, 100_000_020)?;
        timing.observe(0, 200_000_000, 50_200_000_010, 200_000_020)?;
        assert_eq!(timing.capture(0, 50_150_000_000)?, 150_000_000);
        assert_eq!(timing.held("ar-session", 50_150_000_000)?, 150_000_000);
        let mut retained = RetainedTimestamp::default();
        assert_eq!(retained.convert(&timing, "ar-session", 50_150_000_000)?, 150_000_000);
        assert!(timing.capture(1, 50_150_000_000).is_err(), "Screen needs its own measured clock");
        for second in 1..=12 {
            let at = second * 1_000_000_000;
            timing.observe(0, at, 50_000_000_000 + at + 10, at + 20)?;
        }
        assert!(timing.capture(0, 50_150_000_000).is_err(), "Old dynamic capture must not extrapolate");
        assert_eq!(timing.held("ar-session", 50_150_000_000)?, 150_000_000);
        assert_eq!(retained.convert(&timing, "ar-session", 50_150_000_000)?, 150_000_000);
        assert!(retained.convert(&timing, "ar-session", 50_160_000_000).is_err(), "No conversion fabricated for another expired capture");
        assert!(retained.convert(&CaptureTiming::new("phone"), "ar-session", 50_150_000_000).is_err(), "A new SDK clock must not reuse the old conversion");
        Ok(())
    }
    #[test]
    fn missing_unknown_and_reset_native_clocks_are_not_relabelled() -> Result<()> {
        let timing = CaptureTiming::new("phone");
        assert!(timing.capture(0, 12).is_err());
        assert!(timing.observe(2, 1, 2, 3).is_err());
        assert!(timing.observe(0, 1, 2, 2_000_001).is_err());
        timing.observe(0, 100, 1000, 120)?;
        assert!(timing.observe(0, 200, 10_000_000, 220).is_err());
        let replacement = CaptureTiming::new("phone");
        assert_ne!(timing.clock.registry_entry(), replacement.clock.registry_entry());
        assert_ne!(timing.generation, replacement.generation);
        Ok(())
    }
    #[test]
    fn separate_native_ar_and_screen_clocks_share_one_declared_sdk_clock() -> Result<()> {
        let timing = CaptureTiming::new("phone");
        let reference = crate::model::clock_ref(&timing.clock.registry_entry());
        for (kind, epoch) in [(0, 50_000_000_000), (1, 80_000_000_000)] {
            timing.observe(kind, 100_000_000, epoch + 100_000_010, 100_000_020)?;
            timing.observe(kind, 200_000_000, epoch + 200_000_010, 200_000_020)?;
            assert_eq!(timing.capture(kind, epoch + 150_000_000)?, 150_000_000);
        }
        assert_eq!(reference, crate::model::clock_ref(&timing.clock.registry_entry()));
        let utc = crate::capture_time::Contract::local(timing.clock.registry_entry());
        assert_eq!(crate::model::clock_ref(&utc.from_clock), reference);
        assert_eq!(timing.held("ar", 50_150_000_000)?, 150_000_000);
        assert!(timing.observe(1, 300_000_000, 90_300_000_010, 300_000_020).is_err());
        assert!(timing.capture(0, 50_150_000_000).is_err(), "No stream may use a retired session relationship");
        assert!(timing.held("ar", 50_150_000_000).is_err(), "A cached held time cannot hide clock reset");
        assert_ne!(reference, crate::model::clock_ref(&CaptureTiming::new("phone").clock.registry_entry()));
        Ok(())
    }
    #[test]
    fn current_placement_can_assert_a_baseline_but_old_captures_cannot() -> Result<()> {
        let timing = CaptureTiming::new("phone");
        assert!(timing.assert_current_held("ar", 49_900_000_000).is_err());
        timing.observe(0, 100_000_000, 50_100_000_010, 100_000_020)?;
        let before = timing.clock.now_ns();
        let asserted = timing.assert_current_held("ar", 49_900_000_000)?;
        assert!(asserted >= before && asserted <= timing.clock.now_ns());
        assert!(timing.capture(0, 49_900_000_000).is_err(), "Not a substitute capture timestamp");
        assert_eq!(timing.held("ar", 49_900_000_000)?, asserted);
        timing.observe(0, 200_000_000, 50_200_000_010, 200_000_020)?;
        for second in 1..=12 {
            let at = second * 1_000_000_000;
            timing.observe(0, at, 50_000_000_000 + at + 10, at + 20)?;
        }
        assert!(timing.assert_current_held("ar", 50_150_000_000).is_err(), "Expired history is not pre-session evidence");
        Ok(())
    }
}
