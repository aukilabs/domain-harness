//! The BBOS shared-memory interfaces timestamp capture with this host's realtime
//! clock. Map those native readings into the peer's one SDK session clock before
//! creating observations. This is local clock measurement, not network receipt
//! time and not synchronization with another host.
use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct NativeCaptureClock {
    session: auki_time::SessionClock,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    mapping: crate::capture_time::NativeMapping,
    last_warning: Option<Instant>,
}

impl NativeCaptureClock {
    pub fn new(session: auki_time::SessionClock) -> Self {
        Self { session, state: Arc::new(Mutex::new(State::default())) }
    }

    fn observe(&self, state: &mut State) -> Result<bool> {
        let result = state.mapping.observe(&self.session, || {
            auki_time::Clock::read_to_ns(&auki_time::SystemClock) as u64
        });
        match result {
            Ok(()) => Ok(true),
            Err(error) if error.downcast_ref::<crate::capture_time::NativeClockReset>().is_some() => Err(error),
            Err(error) => {
                // A preempted clock read is not a frame reset. Retry on the next
                // tick/capture, and do not publish with an imprecise relationship.
                Self::warn(state, &error);
                Ok(false)
            }
        }
    }

    fn warn(state: &mut State, error: &anyhow::Error) {
        if state.last_warning.is_none_or(|at| at.elapsed() >= Duration::from_secs(5)) {
            crate::diagnostic_log!("Capture clock waiting: {error}");
            state.last_warning = Some(Instant::now());
        }
    }

    pub fn sample(&self) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("Capture clock unavailable"))?;
        self.observe(&mut state)?;
        Ok(())
    }

    /// Frames before this session, outside measured history or ambiguously
    /// timed are dropped. The original processing/IPC delay is preserved.
    pub fn convert(&self, native_timestamp_ns: u64) -> Result<Option<u64>> {
        let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("Capture clock unavailable"))?;
        if !self.observe(&mut state)? { return Ok(None); }
        match state.mapping.convert(native_timestamp_ns) {
            Ok(timestamp) => Ok(Some(timestamp)),
            Err(error) => {
                Self::warn(&mut state, &error);
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbos_utc_capture_delay_is_preserved_for_every_sensor() {
        let session = auki_time::SessionClock::new("robot", "session", "capture");
        let clock = NativeCaptureClock::new(session);
        let epoch = 1_800_000_000_000_000_000;
        let mut state = clock.state.lock().unwrap();
        state.mapping.observe_pair(100, epoch + 100, 100).unwrap();
        state.mapping.observe_pair(900, epoch + 900, 900).unwrap();
        // A camera, pose and cloud captured together remain capture-matched,
        // even if processed after the latter observation.
        for _sensor in ["camera", "pose", "depth", "joints"] {
            assert_eq!(state.mapping.convert(epoch + 250).unwrap(), 250);
        }
        assert!(state.mapping.convert(epoch + 99).is_err());
        assert!(state.mapping.convert(epoch + 901).is_err());
    }

    #[test]
    fn bbos_wall_clock_step_requires_a_new_session() {
        let mut map = crate::capture_time::NativeMapping::default();
        map.observe_pair(100, 1_000_000_100, 100).unwrap();
        let error = map.observe_pair(200, 2_000_000_200, 200).unwrap_err();
        assert!(error.downcast_ref::<crate::capture_time::NativeClockReset>().is_some());
        assert!(map.convert(2_000_000_150).is_err());
    }
}
