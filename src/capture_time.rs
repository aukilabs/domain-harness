//! Registry-bound SDK time transforms transported as ordinary Component Products.
//! A local realtime clock is not proof of synchronization with another host.
use anyhow::{Result, ensure};
use auki_components::*;
use auki_registry::{ClockBody, ClockMeta, ClockRegistryEntry, Scope};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const SCHEMA: &str = "spatial-harness.time-transform/v1";
pub const METADATA: &str = "spatial-harness.time-transform-contract/v1";
pub const MAX_SAMPLE_GAP_NS: u64 = 3_000_000_000;
pub const MAX_CLOCK_STEP_NS: u64 = 5_000_000;
pub const CAPTURE_CONVERSION_BUDGET_NS: u64 = MAX_CLOCK_STEP_NS + 1_000_000;
/// Maximum accumulated clock-conversion error accepted for AR capture matching.
/// This includes both native bridges, not just the UTC sampler's read window.
pub const MAX_CAPTURE_MATCH_UNCERTAINTY_NS: u64 = 20_000_000;

fn reference(clock: &ClockRegistryEntry) -> ClockReference {
    ClockReference {
        peer_id: clock.peer_id.clone(),
        id: clock.clock_id.clone(),
        hash: clock.hash(),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub from_clock: ClockRegistryEntry,
    pub to_clock: ClockRegistryEntry,
    pub source: String,
    /// Fixed for this session. Clock corrections never rewrite old captures.
    pub lifetime: String,
    /// None means the host's UTC accuracy has not been measured. The SDK
    /// sampler's read-window uncertainty is NOT a wall-clock accuracy estimate.
    pub utc_accuracy_ns: Option<u64>,
    /// Conservative additional budget admitted by the bounded native->session
    /// adapter (sampling window + offset variation). Not sensor clock accuracy,
    /// network latency, or proof of cross-device synchronization.
    pub capture_conversion_uncertainty_ns: u64,
}
impl Contract {
    pub fn local(from: ClockRegistryEntry) -> Self {
        let to = ClockRegistryEntry {
            peer_id: from.peer_id.clone(),
            session_id: from.session_id.clone(),
            clock_id: format!("{}/{}/utc", from.peer_id, from.session_id),
            body: ClockBody::UtcClock(ClockMeta {
                unit: "nanoseconds".into(),
                monotonic: false,
                epoch: Some("1970-01-01T00:00:00Z".into()),
                scope: Scope::DeviceLocal,
            }),
        };
        Self {
            from_clock: from,
            to_clock: to,
            source: "session_start_utc_read".into(),
            lifetime: "session".into(),
            utc_accuracy_ns: None,
            capture_conversion_uncertainty_ns: CAPTURE_CONVERSION_BUDGET_NS,
        }
    }
    pub fn output_id(&self) -> String {
        format!("time-{}", manifest_hash(self))
    }
    pub fn validate(&self, output: &OutputManifest) -> Result<()> {
        output.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            self.source == "session_start_utc_read" && self.lifetime == "session",
            "Unsupported time-transform source"
        );
        ensure!(
            self.from_clock.peer_id == output.peer_id
                && self.to_clock.peer_id == output.peer_id
                && self.from_clock.session_id == self.to_clock.session_id
                && !self.from_clock.session_id.is_empty(),
            "Time-transform authority/session mismatch"
        );
        ensure!(
            matches!(&self.from_clock.body, ClockBody::MonotonicClock(m)
            if m.epoch.is_none() && m.monotonic && matches!(m.unit.as_str(),"ns"|"nanoseconds")
            && m.scope==Scope::DeviceLocal),
            "Time-transform source must be a registered session-monotonic clock"
        );
        ensure!(
            matches!(&self.to_clock.body, ClockBody::UtcClock(m)
            if m.epoch.as_deref()==Some("1970-01-01T00:00:00Z") && m.unit=="nanoseconds"),
            "Time-transform destination must declare Unix UTC nanoseconds"
        );
        ensure!(
            output.clock == reference(&self.from_clock) && output.output_id == self.output_id(),
            "Time-transform clock/descriptor mismatch"
        );
        ensure!(
            matches!(&output.payload, PayloadContract::Structured(p)
            if p.datatype==SCHEMA && p.schema==SCHEMA),
            "Wrong time-transform payload"
        );
        Ok(())
    }
}

/// The SDK TimeTransformEntry's fields. Observation time belongs to the SDK
/// envelope, and endpoints/source belong to the immutable descriptor.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub offset_ns: i64,
    pub uncertainty_ns: u32,
}
impl ContractType for Sample {
    const DATATYPE: &'static str = SCHEMA;
}

pub struct Publisher {
    pub contract: Contract,
    _component: Component,
    output: ConfiguredObservable<Sample>,
    capture: BufferProductCapture<Sample>,
    last: Option<u64>,
}
impl Publisher {
    pub fn anchor(&self) -> Result<(u64, Sample)> {
        let value = self.capture.product().latest_existing()?
            .ok_or_else(|| anyhow::anyhow!("Session UTC anchor is not published"))?;
        ensure!(value.sequence == 0, "Session UTC anchor was replaced");
        Ok((value.timestamp_ns, (*value.payload).clone()))
    }
    pub fn mount(
        runtime: &ComponentRuntime,
        endpoint: &auki_component_protocol::ComponentProtocolEndpoint,
        from: ClockRegistryEntry,
    ) -> Result<Self> {
        let contract = Contract::local(from);
        let id = contract.output_id();
        let component = runtime.component(ComponentSpec::new(id.clone()).observable(
            ObservableContract {
                name: "clock".into(),
                datatype: SCHEMA.into(),
                schema: SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            },
        ))?;
        let output = component.configured_observable(ConfiguredObservableSpec::new(
            "clock",
            id.clone(),
            reference(&contract.from_clock),
            PayloadContract::Structured(StructuredPayloadContract {
                modality: "time_transform".into(),
                datatype: SCHEMA.into(),
                schema: SCHEMA.into(),
                observes: "capture_clock_to_local_utc".into(),
                unit: Some("nanoseconds".into()),
            }),
        ))?;
        contract.validate(&output.manifest())?;
        component.expose()?;
        let descriptor = contract.clone();
        let capture = runtime.capture_buffer_with_metadata(
            id,
            &output,
            BufferLimits {
                max_entries: Some(1),
                max_bytes: Some(16384),
                target_duration: None,
            },
            |_: &Sample| 16,
            move |o| {
                Ok(Some(CatalogProductMetadata {
                    schema: METADATA.into(),
                    source_sequence: o.sequence,
                    value: serde_json::to_value(&descriptor).map_err(|e| e.to_string())?,
                }))
            },
        )?;
        endpoint.export_product(&capture.product())?;
        Ok(Self {
            contract,
            _component: component,
            output,
            capture,
            last: None,
        })
    }
    /// The supplied reader MUST read this exact capture clock now, not return
    /// an earlier sensor capture timestamp. Publish one anchor at session start;
    /// subsequent calls do not resample or move that session's UTC relationship.
    pub fn sample(&mut self, clock: &dyn auki_time::Clock) -> Result<bool> {
        if self.last.is_some() {
            return Ok(false);
        }
        let (at, sample) = auki_time::tick(clock);
        ensure!(at >= 0, "Negative capture clock");
        let at = at as u64;
        self.output.publish(
            at,
            Arc::new(Sample {
                offset_ns: sample.offset_ns,
                uncertainty_ns: sample.uncertainty_ns,
            }),
        )?;
        ensure!(
            self.capture.errors().is_empty(),
            "Time-transform retention failed"
        );
        self.last = Some(at);
        Ok(true)
    }
}

/// Sample the SDK session clock against this host's realtime clock. Clones of
/// SessionClock share its epoch; an independent Instant must not replace it.
pub struct SessionUtcClock(pub auki_time::SessionClock);
impl auki_time::Clock for SessionUtcClock {
    fn read_from_ns(&self) -> i64 {
        self.0.now_i64_ns()
    }
    fn read_to_ns(&self) -> i64 {
        auki_time::Clock::read_to_ns(&auki_time::SystemClock)
    }
}

/// Producer-side bridge from a known native capture clock to the SDK session
/// clock. Read the native clock NOW between session reads, never a queued frame's
/// timestamp. Conversion is bounded by measurements, including across delays.
#[derive(Default)]
pub struct NativeMapping {
    samples: std::collections::VecDeque<(u64, Sample)>,
    reset: bool,
}

#[derive(Debug)]
pub struct NativeClockReset;
impl std::fmt::Display for NativeClockReset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Native clock relationship changed; start a new capture session")
    }
}
impl std::error::Error for NativeClockReset {}
impl NativeMapping {
    pub fn observe(
        &mut self,
        session: &auki_time::SessionClock,
        native_now: impl FnOnce() -> u64,
    ) -> Result<()> {
        let before = session.now_ns();
        let native = native_now();
        let after = session.now_ns();
        self.observe_pair(before, native, after)
    }

    /// For a native bridge that performs the same three reads across an FFI
    /// boundary. Both bounding reads must come from this exact SessionClock.
    pub fn observe_pair(&mut self, before: u64, native: u64, after: u64) -> Result<()> {
        if self.reset {
            return Err(NativeClockReset.into());
        }
        ensure!(
            after >= before && after - before <= 1_000_000,
            "Native clock sampling window exceeds 1 ms"
        );
        let timestamp = before + (after - before) / 2;
        ensure!(
            self.samples.back().is_none_or(|s| timestamp > s.0),
            "Native clock sampling is not monotonic"
        );
        let offset_ns = i64::try_from(i128::from(native) - i128::from(timestamp))?;
        if self
            .samples
            .back()
            .is_some_and(|(_, sample)| sample.offset_ns.abs_diff(offset_ns) > MAX_CLOCK_STEP_NS)
        {
            self.reset = true;
            return Err(NativeClockReset.into());
        }
        self.samples.push_back((
            timestamp,
            Sample {
                offset_ns,
                uncertainty_ns: (after - before) as u32,
            },
        ));
        // At most ten seconds plus one predecessor, with a hard cap even when
        // capture callbacks are unusually frequent.
        while self.samples.len() > 4096
            || (self.samples.len() > 2
                && timestamp.saturating_sub(self.samples[1].0) > 10_000_000_000)
        {
            self.samples.pop_front();
        }
        Ok(())
    }

    pub fn convert(&self, native: u64) -> Result<u64> {
        if self.reset {
            return Err(NativeClockReset.into());
        }
        let samples = self.samples.iter().cloned().collect::<Vec<_>>();
        inverse(&samples, native).map(|(timestamp, _)| timestamp)
    }
}

/// Invert a sampled offset relationship without extrapolation. Native clock
/// steps can produce overlapping windows; ambiguous captures must be rejected.
pub fn inverse(samples: &[(u64, Sample)], native: u64) -> Result<(u64, u64)> {
    ensure!(
        samples.windows(2).all(|w| w[0].0 < w[1].0),
        "Unordered time-transform history"
    );
    let mut found: Option<(u64, u64)> = None;
    for pair in samples.windows(2) {
        let start = i128::from(pair[0].0) + i128::from(pair[0].1.offset_ns);
        let end = i128::from(pair[1].0) + i128::from(pair[1].1.offset_ns);
        if !(start..=end).contains(&i128::from(native)) {
            continue;
        }
        ensure!(
            pair[1].0 - pair[0].0 <= MAX_SAMPLE_GAP_NS,
            "Time-transform history gap"
        );
        let drift = pair[0].1.offset_ns.abs_diff(pair[1].1.offset_ns);
        ensure!(drift <= MAX_CLOCK_STEP_NS, "UTC clock step at capture");
        // Interpolate the measured relationship with integer arithmetic, not
        // floating point on epoch-sized values. Exact endpoints remain exact.
        let span = end - start;
        ensure!(span > 0, "Native capture clock did not advance");
        let session = i128::from(pair[0].0)
            + (i128::from(native) - start) * i128::from(pair[1].0 - pair[0].0) / span;
        let mut value = (
            u64::try_from(session)?,
            drift + u64::from(pair[0].1.uncertainty_ns.max(pair[1].1.uncertainty_ns)),
        );
        if let Some(previous) = found {
            ensure!(
                previous == value || previous.0 == value.0,
                "Ambiguous native clock after reset"
            );
            // Adjacent intervals share exact endpoints. Their error budgets
            // may differ; selecting that endpoint must not discard the larger
            // bound merely because the lower-error interval was visited last.
            value.1 = value.1.max(previous.1);
        }
        found = Some(value);
    }
    found.ok_or_else(|| anyhow::anyhow!("Waiting for capture-matched time transforms"))
}

/// Apply the held session-start UTC relationship. This maps clock readings,
/// not poses: no fresh UTC sample is needed for each capture. It intentionally
/// does not follow subsequent corrections of the host's realtime clock.
pub fn to_utc(
    contract: &Contract,
    samples: &[(u64, Sample)],
    timestamp: u64,
) -> Result<(u64, u64)> {
    ensure!(
        contract.lifetime == "session",
        "Unsupported time-transform lifetime"
    );
    ensure!(
        samples.len() == 1,
        "Expected one held session-start UTC transform"
    );
    let before = &samples[0];
    // The startup read establishes the session's epoch, including clock zero;
    // its observation time is not a validity cutoff for this held relationship.
    let uncertainty = u64::from(before.1.uncertainty_ns);
    let transform = auki_time::TimeTransform::new(
        serde_json::to_string(&reference(&contract.from_clock))?,
        serde_json::to_string(&reference(&contract.to_clock))?,
        before.1.offset_ns,
        uncertainty,
        i64::try_from(before.0)?,
    );
    let converted = transform
        .convert_ns(i64::try_from(timestamp)?)
        .ok_or_else(|| anyhow::anyhow!("Time conversion overflow"))?;
    Ok((u64::try_from(converted)?, uncertainty))
}

pub fn from_utc(contract: &Contract, samples: &[(u64, Sample)], utc: u64) -> Result<(u64, u64)> {
    ensure!(
        samples.len() == 1,
        "Expected one held session-start UTC transform"
    );
    let timestamp = u64::try_from(i128::from(utc) - i128::from(samples[0].1.offset_ns))?;
    let (round_trip, uncertainty) = to_utc(contract, samples, timestamp)?;
    ensure!(round_trip == utc, "UTC conversion mismatch");
    Ok((timestamp, uncertainty))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contract() -> Contract {
        Contract::local(ClockRegistryEntry {
            peer_id: "peer".into(),
            session_id: "session".into(),
            clock_id: "capture".into(),
            body: ClockBody::MonotonicClock(ClockMeta {
                unit: "nanoseconds".into(),
                monotonic: true,
                epoch: None,
                scope: Scope::DeviceLocal,
            }),
        })
    }
    fn sample(offset: i64) -> Sample {
        Sample {
            offset_ns: offset,
            uncertainty_ns: 20,
        }
    }
    #[test]
    fn session_anchor_is_held_and_does_not_claim_global_accuracy() {
        let c = contract();
        let samples = [(100, sample(1000))];
        assert_eq!(to_utc(&c, &samples, 150).unwrap(), (1150, 20));
        assert_eq!(
            to_utc(&c, &samples, 20_000_000_000).unwrap(),
            (20_000_001_000, 20)
        );
        assert_eq!(from_utc(&c, &samples, 1150).unwrap(), (150, 20));
        assert!(c.utc_accuracy_ns.is_none());
        assert_eq!(to_utc(&c, &samples, 0).unwrap(), (1000, 20));
        assert!(from_utc(&c, &samples, 999).is_err());
        assert!(to_utc(&c, &[(100, sample(0)), (200, sample(6_000_000))], 150).is_err());
        assert!(to_utc(&c, &[(100, sample(-1000))], 100).is_err());
    }
    #[test]
    fn session_changes_change_the_clock_contract() {
        let a = contract();
        let mut b = a.clone();
        b.from_clock.session_id = "reset".into();
        assert_ne!(a.output_id(), b.output_id());
        assert_ne!(reference(&a.from_clock), reference(&b.from_clock));
    }
    #[test]
    fn delayed_native_capture_keeps_its_capture_time() {
        let mut mapping = NativeMapping::default();
        mapping.observe_pair(1_000, 1_001_000, 1_000).unwrap();
        mapping.observe_pair(2_000, 1_002_000, 2_000).unwrap();
        assert_eq!(mapping.convert(1_001_250).unwrap(), 1_250);
        assert!(mapping.convert(1_000_999).is_err());
        assert!(mapping.convert(1_002_001).is_err());
        assert!(mapping.observe_pair(3_000, 1_003_000, 2_999).is_err());
        assert!(mapping.observe_pair(3_000, 1_003_000, 2_000_000).is_err());
        let reset = mapping.observe_pair(3_000, 10_003_000, 3_000).unwrap_err();
        assert!(reset.downcast_ref::<NativeClockReset>().is_some());
        // Returning to the old offset must not revive the same clock identity.
        assert!(
            mapping
                .observe_pair(4_000, 1_004_000, 4_000)
                .unwrap_err()
                .downcast_ref::<NativeClockReset>()
                .is_some()
        );
        assert!(
            mapping
                .convert(1_001_250)
                .unwrap_err()
                .downcast_ref::<NativeClockReset>()
                .is_some()
        );
    }
    #[test]
    fn native_mapping_evicts_old_history_but_never_extrapolates() {
        let mut mapping = NativeMapping::default();
        for t in 1..=20 {
            let t = t * 1_000_000_000;
            mapping.observe_pair(t, t + 1_000_000_000_000, t).unwrap();
        }
        assert!(mapping.convert(1_001_000_000_000).is_err());
        assert_eq!(mapping.convert(1_019_500_000_000).unwrap(), 19_500_000_000);
        assert!(mapping.samples.len() <= 12);
    }
    #[test]
    fn inverse_shared_endpoint_preserves_the_larger_uncertainty() {
        let mut earlier = sample(1_000);
        earlier.uncertainty_ns = 400;
        let samples = [(100, earlier), (200, sample(1_004)), (300, sample(1_004))];
        assert_eq!(inverse(&samples, 1_204).unwrap(), (200, 404));
    }
    #[test]
    fn clocks_in_one_session_share_utc_but_other_sessions_do_not() {
        let a = auki_time::SessionClock::new("peer", "first", "capture");
        let b = auki_time::SessionClock::new("peer", "first", "other");
        let c = auki_time::SessionClock::new("peer", "second", "capture");
        assert_eq!(
            Contract::local(a.registry_entry()).to_clock,
            Contract::local(b.registry_entry()).to_clock
        );
        assert_ne!(
            Contract::local(a.registry_entry()).to_clock,
            Contract::local(c.registry_entry()).to_clock
        );
        assert_eq!(a.registry_entry(), a.clone().registry_entry());
    }
}
