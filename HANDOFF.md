# Home, phones, and Bracketbot handoff

This is a draft integration snapshot of the work since PR #3. Native Android and
iPhone changes are included deliberately. Temporary scratch files, credentials,
runtime state, recordings, generated binaries, and dependency caches are excluded.

## Review areas

- **Robot:** Bracketbot runner; head and wrist camera Products and
  portal detection; capture-time RGB/depth/pose joins; tracking and PGO correction
  barriers; bounded independent acquisition and mapping; scan-progress diagnostics.
- **Home:** independent saved reference scenes and scene-scoped collaboration;
  selected-scene pairing; correct robot display axes; duplicate scene-row removal;
  cameras grouped in the component catalog; phone pose trails and UI cleanup.
- **Phones:** Android portal calibration and bounded background scan work;
  shared Rust camera/depth/local-map Products; demand-driven camera preview;
  authenticated note changes; iPhone selected-scene pairing and bridge updates.

Start with `bracketbot-runner.md`, `android/README.md`, and
`ios/PortalViewer/README.md`. Some older device-check notes describe earlier
milestones, not verification of this exact snapshot.

## Robot deployment: confirmed versus unverified

The last confirmed installed runner binary had SHA-256:

`a8125aa959abb54cabde6407aca08557dbad18919597910e4685c2188c1e8613`

That build included the PGO correction barrier but **not** the latest independent
capture worker and `local_mapping.scan_progress` diagnostics. Last observed runner
PID was 17864, supervisor PID 17863, session
`4a6002fc-c6bb-405c-82ff-eba73eab8063`. These are historical observations, not
current health claims. Only two scans had integrated in that session.

The newer source was uploaded by a build attempt, but remote build/install
completion was not confirmed. The local hung build/SSH clients were stopped;
the remote compiler's eventual outcome is unknown. Later SSH connections timed
out during banner exchange while the robot still answered ping. The user reported
no visible problem with the robot. This is an unresolved deployment-channel issue,
not evidence of hardware failure. No final restart or hardware validation of the
new capture implementation has been performed.

An independent BBOS probe previously saw fresh JPEG, point, and pose frames with
matching timestamps. The new acquisition worker addresses inline image processing
delaying pose ingestion, but that explanation still needs confirmation on hardware.
Do not present it as a proven root cause or blame QR Lab decoding.

PGO corrections retain the peer/session, invalidate alignment and pending old-epoch
measurements, and rebuild geometry from new scans. They do not reconstruct the
historical map. The supervisor should not restart merely because `pgo_count` changes.

## Next robot checks

1. Restore SSH access to robot 0184 (`192.168.1.118`) and inspect processes plus
   `/home/bracketbot/deployments/domain-harness/runner/build-manifest.json` before
   deciding whether another build is needed.
2. Build from this checkout with `python3 scripts/build-bracketbot-runner.py
   --code-only` only if its vendored dependencies already exist on the robot.
   Building and starting are separate operations.
3. Use `scripts/run-bracketbot-runner.py --restart --home-connection <profile>`
   with the existing credential wrapper and the selected Conference scene profile.
   Keep **one runner**, not simultaneous old/new runners. Do not commit the
   operator's credential files or private identity.
4. Confirm the installed binary hash, one runner/supervisor, and fresh advancing
   `local_mapping.scan_progress` counters in status and Home snapshots. Observe
   poses, integrated scans, pose-match expiry, and scan age separately.
5. Verify all four camera feeds, portal observations, fresh map updates, and a PGO
   correction without a PID/session restart. Do not move the robot or reset SLAM
   as an implicit part of software verification.

## Reference and calibration caveat

The required reference is `local-scene:humanoid-hub-conference:portal-HNY3JGNLRAE`,
served by `local-scene:humanoid-hub-conference:mobile`, not the default Xmart
reference. Portal HNY3JGNLRAE has a 15 cm encoded square. Its saved reference is
centered at the origin on the floor, face +Y, right +X, top -Z. It was physically
moved to a table during troubleshooting. Match the physical placement to the saved
reference, or explicitly agree on a reference update, before claiming calibrated
global alignment. No alignment was confirmed in the last observed robot session.

## Verification scope

The PR description records tests run on this isolated snapshot. Automated checks
do not establish current robot deployment, end-to-end phone AR performance,
real-device camera behavior, or physical portal calibration. Those remain explicit
hardware checks for the next operator.
