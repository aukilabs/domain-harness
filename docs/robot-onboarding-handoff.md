# Robot onboarding: handoff (2026-10-15)

Branch `robot-onboarding-next`, cut from `develop` at `09ee5ce`. Everything
below is already merged into `develop`; this branch is for the next step:
making it work on a real robot. Spec: `onboarding.md`.

## What exists (all merged)

| Piece | Where | PR |
| --- | --- | --- |
| Shared logic: invitation format, marker payload, per-phone screen geometry, clock mapping, pose history, solve, agreement | `src/onboarding.rs` | #68 |
| Square (marker/Portal) pose solver, shared by phone and robot | `src/square_pose.rs` | #73 |
| Phone: host (shows QR, then the 50 mm marker; answers `show` / `clock` / `marker_at` / `progress`) and phone joiner | `src/mobile_onboarding.rs`, `src/mobile.rs`, `ios/PortalViewer/App/Onboarding.swift` | #68 |
| iPhone permissions list (camera, location, notifications, local network, screen recording, in order) | `ios/PortalViewer/App/McKennaIntro.swift` | #71 |
| Robot joiner in the Diagnostics runner (`--portals`) | `diagnostics/connection-peer/src/onboard.rs`, hooks in `camera.rs` and `alignment.rs`, README section | #73 |
| Connection reuse (onboarding exchanges no longer dial per request) | `vendor/auki-p2p/src/transport.rs` | #69 |

## How the robot side works

1. The head-camera detector (`camera.rs`) hands every onboarding code
   (invitation JSON starting `{"schema":"spatial-harness.onboarding/v1"`, or an
   `ONB:` marker) to `onboard::offer` over an in-process channel. Never in
   the published detections: an invitation can hold a Wi-Fi password.
2. `onboard::run` (spawned in `main.rs` when `--portals`) parses the
   invitation, ignores repeats of a session, and refuses one for a different
   Home (same Home peer and scene frame required).
3. Wi-Fi: `sudo -n nmcli dev wifi connect <ssid> [password …]`, no shell,
   previous network kept on failure. Skipped if already on that SSID.
4. Names itself in Home: `home-phone-presence:<scene>` `name`, alias bound to
   the robot's Peer ID.
5. Asks the phone (`onboarding-display` Component) to `show` its marker, then
   16 `clock` exchanges; the tightest gives the phone-clock offset ± half its
   round trip. Robot capture clock = wall-clock ns (BBOS stamps frames that way).
6. Each marker sighting: undistort the 4 corners with the fisheye model,
   solve the 50 mm square (OpenGL camera → optical), ask the phone
   `marker_at(t_phone)`, giving "where this camera was in the scene".
7. The alignment worker composes that with the capture-time base pose and
   the head extrinsic, feeds `onboarding::Agreement` (5 sightings within
   2 cm / 2°, timing error ≤ 1 cm), and publishes the agreed transform on the
   normal held alignment Product (the one a Portal alignment uses). Then the
   phone gets `progress: aligned` and plays its success sound.

## Never tested on hardware

- No robot has run this. No phone→phone onboarding has completed end to end.
- Tests pass (Rust suite, Diagnostics 42/42) but they are unit/fixture tests.

## Before the first robot try

1. **sudoers** on the robot for exactly `nmcli dev wifi connect …` for the
   runner's user, or the Wi-Fi step fails (it says so; everything else runs).
   Skip Wi-Fi in the invitation if the robot is already on the network.
2. Run the Diagnostics runner with `--portals` and the **same Home profile**
   the phone is paired to (`scripts/run-bracketbot-connection-diagnostic.py
   --portals --home-connection …`).
3. Robot needs its existing SDK credentials for that Domain (no enrollment
   of a factory-fresh robot exists; see `onboarding.md` § Authentication).
4. Phone: open **+**, enter alias, calibrate on a Portal, show the QR to the
   robot head camera, then hold the phone still showing the 50 mm marker.
5. Watch the runner log for `Onboarding as …`, `clock matched to ±… ms`,
   `Onboarding sighting …`, `Alignment established … from an onboarding marker`.

## Known gaps / likely first bugs

- **Screen geometry is estimated** (`SCREENS` in `src/onboarding.rs`,
  `verified: false`): camera position behind the screen per iPhone model, 1 cm
  deep. A few mm–1 cm error is expected. Measure: onboard a phone that can
  also see a real Portal and compare the two alignments.
- **Marker size**: 50 mm (not the 30 mm in `onboarding.md`; fisheye needs it).
  If the robot cannot read it at distance, move closer or raise `MARKER_SIDE_M`.
- **Fisheye corner undistortion** reuses `fisheye::UnrectifiedEye::rectify`;
  the solve assumes the rectified pinhole with the same K. Check reprojection
  if the pose looks off.
- **Clock**: robot uses wall clock, phone uses ARKit uptime; mapping is by
  round trips. Large Wi-Fi jitter → "phone answers too slowly" (limit ±400 ms)
  or sightings rejected for timing error. Hold the phone still.
- **Success** fires on the joiner's report, not on Home confirming the
  robot's pose (spec step 4). Add a Home "resolve this peer's pose" check.
- Invitation for another Home is refused rather than switching profiles.
- Android cannot host yet (no screen table, no `capture_clock_now`).
- Detector only offers codes it already decodes; the 50 mm `ONB:` code is
  version-1 QR (21 modules) so it should decode where Portals do.

## Next work for the robot runner

1. Deploy the Diagnostics runner with `--portals` on one robot (0184/182),
   add the sudoers entry, and do the first end-to-end try with logs.
2. Fix what that shows (most likely timing tolerance and marker distance).
3. Home-side confirmation before the phone's success sound.
4. Measure and mark verified the screen geometry for the demo phones.
5. If the old `src/robot_runner.rs` path is ever needed again, port
   `onboard.rs` there; robots currently run the Diagnostics runner.
