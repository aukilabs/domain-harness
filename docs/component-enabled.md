# Publication enable control

Home discovers this optional capability on any peer's Catalog, not by device
name or component ID. Peer communication uses authenticated SDK operations.

- Operable `set_enabled`: instruction `spatial-harness.set-enabled/v1`, result
  `spatial-harness.component-enabled-state/v1`. Both payloads are `{ "enabled": bool }`.
- A retained state Product from the exact same Component manifest carries
  schema-tagged `spatial-harness.component-enabled-state/v1` Catalog metadata,
  tied to its source sequence. It supplies initial UI state. An operation's
  result confirms the new state; timeout does not imply success or failure.
- The publisher authorizes the caller. The diagnostics depth component permits
  only the Home peer pinned in its connection profile. Domain membership alone
  does not grant control.
- Home invokes the exact discovered Component manifest; unsupported or changed
  components are rejected. Writes are bounded, have deadlines, and are not retried
  automatically. Missing/stale state disables the UI control.

For diagnostics depth, disabled means no new measured-point observations enter
the Product. Publication also requires an active SDK subscription; enable does
not force publication without a consumer. BBOS sensing, camera, poses, and alignment continue. Retained samples
and previously mapped Home voxels are not deleted. Re-enabling resumes fresh
captures without replaying the disabled interval. A runner restart defaults to
enabled; this switch is session state, not a persistent BBOS setting.
