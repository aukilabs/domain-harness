# Bracketbot runner

`bracketbot-runner` runs one authenticated SDK peer on Bracketbot **0184** at
**192.168.1.118**. It publishes a rectified head-camera view, camera calibration
and mount pose, capture-time odometry, synchronized colored stereo observations,
and its independently built local map. It reads BBOS shared memory and does not
own motor/control channels.

The configured Portal is **HNY3JGNLRAE**, with a manually confirmed **0.15 m
encoded-square side length** in `config/humanoid-hub-conference/portals.json`.
Detection uses the SDK QR detector/localizer. The robot begins mapping from its
first valid pose; detecting a Portal does not move that local origin.

The mapper alternates rectified left/right head-camera samples at the existing
combined 2 Hz maximum. Each eye has separate intrinsics, camera products, and
calibration/provenance. The right camera's transform is derived from BBOS's
metric stereo R/T and both rectification rotations, not copied from the left
camera. Each captured depth cloud is integrated only once, expressed in the
selected eye's optical frame. `camera.head` and `camera.head.right` publish both
rectified head-eye previews at up to 5 Hz (JPEG quality 95). The separate
`local-camera-right-history` exposes the right-eye mapping images.

Both head eyes and both wrist JPEG feeds run independent pixel-only SDK Portal detectors:
`camera.head.portal-detector`, `camera.head.right.portal-detector`,
`camera.left.portal-detector` and `camera.right.portal-detector`. Each exports a
`-history` product containing `spatial.landmark-observations/v2`, including empty
detection batches. Corner identities are `SHORTCODE/top_left`, `top_right`,
`bottom_right`, and `bottom_left`, with the producing detector reference and
exact camera output, sequence, timestamp, and clock retained as provenance.
Each worker retains only its newest pending frame so detection cannot stall
camera publishing. All four pixel detectors work without SLAM tracking or depth;
their processed-frame, decoded-QR, Portal, timestamp, and error counters appear
under each sensor in `state/local-scene.status.json`. Mapping scan diagnostics
separately report decoded QRs, accepted map placements, and rejection reasons.
Wrist detection works without calibration;
it does **not** create posed anchors or map positions until wrist calibration
and capture-time transforms are available. Head mapping still requires valid
SLAM poses and synchronized depth; pixel detections alone do not claim alignment.

## Build and launch

### Detection/connection verification — 11 October 2026

Installed runner SHA-256:
`c68918696490a1afd458daaf0a9132d15484849a2dc30adaca1e1aac739fe9e0`.
After a battery-swap reboot, exactly one runner was observed (PID 9331, session
`02124d94-4f8f-413e-9d76-ab3f7b1a8aaa`). It fetched the authenticated
`local-scene:humanoid-hub-conference:portal-HNY3JGNLRAE` reference successfully.
During a 16-second check with SLAM lost, head detector processed-frame counts
advanced 4→13 (left) and 4→15 (right); both wrist counters also advanced.
All four reported zero errors. No new Portal decoded during that check, and no
Home alignment was applied. This verifies independent scanning and pairing,
not physical calibration. The saved Conference reference still assumes the
Portal is at its declared floor pose; no saved pose was changed.

Local validation: 30 library tests, 10 mapper tests (3 recording-dependent tests
ignored), 12 Python capture/launch tests, and the authenticated selected-scene
test passed. A power-interrupted package cache caused a linker failure; only
`spatial-home` Cargo artifacts were cleaned and rebuilt, leaving source,
dependencies, saved scenes and BBOS state untouched.

From the repository root on this Mac:

```sh
python3 scripts/build-bracketbot-runner.py
python3 scripts/with-home-credentials.py python3 scripts/run-bracketbot-runner.py --home-connection data/bracketbot-verification/home-connection-for-robot.json
```

The Home profile must select **Humanoid Hub Conference**, whose reference contains
`HNY3JGNLRAE`, not the default Xmart scene. Obtain that scene profile from
`/api/local-scenes/humanoid-hub-conference/connection` and use Home's LAN route.
The Home profile must identify the current Home peer and a robot-reachable LAN
route (not `127.0.0.1`). Subsequent restarts can reuse the saved profile. Launch
preflight rejects a missing or loopback-only profile before stopping the runner.
Home references are fetched even before any Portal is mapped; connection status
and whether the reference has been applied are reported separately in status.

The first build vendors the locked dependencies locally and builds them on the
ARM64 robot using its installed Rust toolchain. Subsequent source updates can
reuse those dependencies:

```sh
python3 scripts/build-bracketbot-runner.py --code-only
python3 scripts/with-home-credentials.py python3 scripts/run-bracketbot-runner.py --restart
```

Run a full build again after changing `Cargo.lock`. The build installs an atomic
binary replacement and writes `build-manifest.json` with binary, lockfile, and
adapter hashes. The launcher sends credentials over encrypted SSH stdin; they
stay in the supervisor/runner environment and are stripped from capture workers.
Starting after a robot reboot requires the launcher again.

Deployment root: `/home/bracketbot/deployments/domain-harness/runner`.

| Path under the deployment root | Purpose |
| --- | --- |
| `bin/bracketbot-runner` | Installed ARM64 executable. |
| `source/` | Pinned build inputs and Python adapters. |
| `state/peer.identity` | Persistent identity unique to this robot; do not copy to bot2. |
| `state/connection.json` | Public SDK connection profile for Home. |
| `state/local-scene.json` | Latest completed map snapshot. |
| `state/local-scene.status.json` | Tracking status, sensor counts, and freshness. |
| `state/local-scene.catalog.json` | Published SDK component/product catalog. |
| `state/runner.log` | Capture, mapping, and peer diagnostics. |
| `state/runner.pid` | Supervisor PID. |

Stop only this runner with:

```sh
bb ssh 0184 'kill -TERM "$(cat /home/bracketbot/deployments/domain-harness/runner/state/runner.pid)"'
```

The supervisor gives the runner time to close its SDK peer. BBOS continues
running. An unexpected exit restarts the runner with backoff and a fresh local
map session. Previous completed snapshots are archived alongside the current one.

## Home connection

Copy the **public** profile:

```sh
bb ssh 0184 'cat /home/bracketbot/deployments/domain-harness/runner/state/connection.json' > data/bracketbot-0184-connection.json
```

In Home's peer config, add an entry to `runner_scenes` with `name` set to
`bot1 · Bracketbot 0184` and `connection` set to the complete profile object.

Home connects directly to TCP **4321** on `192.168.1.118`; both devices must be
on a reachable network. The profile declares the `bracketbot-local` family, so
Home can reconnect to a new map-session UUID while retaining the robot's peer
identity. Select its `camera.head-history` camera and its independent scene.

The launcher also accepts `--home-connection PATH` to supply a compatible Home
canonical reference profile to the runner. Connecting peers alone does not
establish spatial alignment. The existing canonical-reference pipeline uses
Xmart's reference; conference-scene fusion and phone/robot annotation alignment
must be verified separately before the seven-beat demo. Never relabel the
Bracketbot's local map as that reference frame.

## Measurement contract

- The head camera provides a 2560 × 960 stereo JPEG. The adapter rectifies the
  left eye at **1280 × 960**, using BBOS's fisheye calibration. Camera publication
  targets **5 Hz**; QR detection and colored depth integration target **2 Hz**.
- BBOS computes depth at 512 × 384. Its colored point cloud is already expressed
  in native base axes: **X right, Y forward, Z up**. The adapter inverts the
  configured optical-to-base rigid transform once. An on-device pixel roundtrip
  measured a median error of **0.032 px**; adding a second rectification rotation
  was incorrect.
- JPEG and points must have the **same capture timestamp**. Each integration
  contains at most **384** measured colored points, at 0.2–5 m optical depth and
  at most 5 m range. There is no nearest-frame RGB/depth pairing.
- BBOS acquisition runs independently of JPEG decoding, rectification, point
  transforms, and preview encoding. It copies frames into a bounded 48-capture
  join and publishes dense timestamped odometry directly, so image processing
  cannot intentionally decimate pose brackets to preview frequency. Both head
  preview eyes share one JPEG decode. Mapping still decodes its exact paired
  capture separately; detector JPEG decoding remains a separate transport cost.
- `local_mapping.scan_progress` (also in the runner status file) distinguishes
  integrating, stalled, waiting-for-first-scan, and tracking-unavailable states.
  It reports the last integrated capture timestamp and elapsed integration age,
  pending scans, and counts of expired pose joins, overflow, and tracking-loss
  discards. Capture status separately reports paired/emitted/stale scans and
  admitted pose gaps. A live pose or live network connection is not evidence
  that the voxel map is updating.
- A SLAM pose is published by BBOS only when `pose_valid` is true. Fresh,
  localized poses are accepted, matching BBOS's own mapper. `degraded` is exposed
  as a quality warning. Lost, stalled, stale (>800 ms), or unlocalized tracking
  pauses geometry integration; camera preview can continue.
- BBOS pose-graph corrections are mapper barriers, not process failures. The
  runner keeps its peer, session, camera streams, and independent detectors.
  It invalidates local voxels, observed portal placements, pending scans, and
  Home alignment in place, admitting only scans at/after the corrected pose.
  The fixed coordinate transform is retained; the corrected pose can jump.
  Scene metadata records the correction timestamp/counter and policy. Home
  reference data is retained for alignment after a fresh portal observation.
  This currently rebuilds from new scans, not corrected historical scan replay.
- Calibration changes, producer restarts, and clock discontinuities still end
  the session and cause the supervisor to start a fresh map.
- Correction-handling deployment verified on 2026-10-11: binary SHA-256
  `a8125aa959abb54cabde6407aca08557dbad18919597910e4685c2188c1e8613`.
  One runner (PID 17864) retained session
  `4a6002fc-c6bb-405c-82ff-eba73eab8063` through an initial tracking gap and
  recovery. Home then received fresh poses and all four camera streams.
  The live PGO counter stayed at 27 during this verification interval, so a
  post-deployment correction crossing remains to be checked on hardware.
  Regression checks: 18 Python tests and 42 Rust tests passed; 3 recording-only
  Rust tests were not run. Tests exercise repeated corrections, overload,
  late pre-correction scans, valid snapshot serialization, and recovery.
- The Python adapter uses BBOS's existing depth-daemon runtime environment.
  Dynamic IMU-adjusted camera extrinsics are rejected until a capture-time
  transform stream is available.

## Verification

```sh
python3 -m unittest discover -s tests -p test_bracketbot_capture.py
cargo test --locked --lib --test local_mapper --test network
cargo clippy --locked --lib --bin bracketbot-runner --bin galbot-runner
```

Python tests require NumPy. The tests cover native-frame projection, exact
timestamp pairing, bounded sampling, stale/lost tracking, session resets,
Bracketbot Portal localization, colored room-depth integration, and existing
Galbot/network behavior.

### Live result — 11 October 2026

The runner is installed and running with public peer ID
`12D3KooWDBd1dpRQ8w2WKfT5QgW9ZqKcs5aFPUo7uiQsg7CS2Rwi`.
The installed executable SHA-256 is
`3f388d1024ac87e0ebadf075d38db2fc386e6284650d190c9905c9981080dcb3`.

- Eight Python tests and 40 Rust mapper/library/network tests passed. Clippy
  completed with one existing warning in `src/mobile_local.rs`; the changed
  runner code passed its checks.
- An eight-second live adapter check produced 118 odometry updates, 15 exact
  RGB/depth observations, and 34 camera/transform pairs.
- An authenticated Home at `http://127.0.0.1:4329` received the 1280 × 960 JPEG
  camera with its matching transform Product and explicit intrinsics. A captured
  map contained **148 integrated observations and 856 measured voxels**.
- A controlled runner restart preserved peer identity and Home automatically
  reconnected to the new independent map session.
- During setup, the preexisting SLAM state was backed up to
  `/home/bracketbot/deployments/domain-harness/slam-before-runner-20261011.tar.gz`
  before a graceful SLAM-only restart. No BBOS configuration or motor commands
  were changed.

**Remaining demo blockers:** BBOS tracking later became lost again; the runner
reported `waiting_for_odometry` and kept publishing its camera. BBOS logs cite
blur and failed relocalization consensus. The floor Portal is visible but has
not decoded. Offline checks on the actual image at three rectification scales,
including the SDK's stronger recovery ladder, also produced no decode. The
detector already uses full input resolution. The user requested retaining the
Portal's current position. No unmeasured Portal pose or shared alignment is
published.

The verification Home uses `data/bracketbot-verification/home-peer.json` and an
independent Home identity. Its camera/map checks do not establish shared
conference alignment or verify the phone annotation beats. The copied public
runner profile is `data/bracketbot-0184-connection.json`.

## Wrist cameras and Portal search stance

The BBOS adapter publishes `camera.left-history` and `camera.right-history`
alongside the head camera. Both are raw 640×480 JPEG streams, limited to 5 Hz;
select them under **Devices → Bracketbot → Camera feeds** in the existing Home.
They have distinct optical frames and device capture timestamps. Missing lens
calibration and capture-time wrist-to-base transforms are declared explicitly:
wrist AR and metric Portal localization are not enabled using guessed values.
The calibrated head camera remains the source of metric Portal observations.

`seek-bracketbot-portal.py` supports one-shot and supervised search. It is never
started by the sensor adapter. It saves both actual arm
poses, uses the factory FK model and measured limits to solve a wrist-only
45° forward/down finger direction, moves over eight seconds, then restores
those saved poses over eight seconds. Mast, shoulder, elbow and gripper goals
remain unchanged; the grippers remain unpowered. This implementation accepts
only the verified hanging-arm starting envelope; it rejects arbitrary arm poses.

Read-only plan on robot 0184:

```sh
bb ssh 0184 'cd /home/bracketbot/deployments/domain-harness/runner/source && python3 scripts/bracketbot-python.py --daemon arm_left scripts/seek-bracketbot-portal.py --state ../state'
```

To perform one search/return cycle, append `--execute`. `--max-seconds` bounds
the search hold to at most 300 seconds (default); a shorter hold is useful for
verifying the return motion. A successful calibration requires three new SDK
metric estimates of `HNY3JGNLRAE`, fresh tracking in the same capture session,
and Home alignment through that Portal. A decoded QR alone does not qualify.
Until wrist optics and kinematic extrinsics are measured, pointing the wrists
downward does not add a new metric localization input.

`state/portal-stance.json` records the PID, saved/target poses, phase, measured
hand directions, return errors, and completion reason. SIGINT/SIGTERM requests
a controlled return. Timeout, lost mapping session, or unavailable mapping
also requests a return. Stale arm feedback, unsafe tilt, protection margins,
or another controller taking ownership abort motion and yield to BBOS instead
of attempting an unobserved recovery. No high-priority writers are acquired.
The initial enable phase holds the saved pose while BBOS verifies motor modes;
normal movement requires fresh feedback. Factory protections remain active.

### Automatic behavior controller

The runner supervisor launches `control-bracketbot-behavior.py` in read-only
mode by default. `state/arm-behavior.status.json` exposes its phase and the
separate search/pointing execution gates. After confirming empty hands and a
clear wrist workspace, the launcher flag `--execute-portal-search` enables
supervised wrist search. This flag must be supplied on each launcher restart;
it is intentionally not a persistent motor-enable setting.

Search holds until the mapper publishes a valid Home transform, unless feedback,
temperature, controller ownership, or supervisor health requires an
earlier stop. The helper's one-shot duration limit still applies outside this
supervised mode. The controller requests restoration before another search;
unexpected exits and safety stops latch a fault instead of automatically
retrying. A lost Home transform requests search again with a live runner.
Fixed wrist search uses fresh local arm feedback, not SLAM pose freshness;
it remains available during localization loss. Fresh world pose is required
before considering a phone target, not before entering the fixed search pose.
A QR leaving view does not by itself invalidate an established transform.

`--execute-phone-reach` additionally enables persistent full-arm reaching after
calibration. Home exposes a scene-scoped authenticated `snapshot` operation on
its phone-presence component; the robot caches only that small live response,
with request duration and local receipt time. No HTTP control channel or foreign
clock subtraction is used. Phone age must be at most 2000 ms including request
duration; robot pose at most 800 ms, control snapshot 750 ms, arm feedback 250 ms.
The controller reads `state/robot-control.json`, refreshed every 50 ms from
capture ingress independently of QR decoding, voxel integration and scene-file
writes. The native origin and alignment still come from the mapper. Tracking
loss immediately pauses reaching. BBOS recovery into the same map resumes the
existing alignment; an actual map correction still invalidates it until new
portal evidence. A mapper basis older than three seconds is also rejected.
Phone discovery is cached by the complete Home profile, but every pose read is
a new authenticated invocation. Profile changes, failures and timeouts discard
discovery; freshness includes invocation RTT, not the earlier catalog lookup.

The nearest localized phone is selected by robot-origin distance; the nearest
measured hand chooses the arm. The mast and gripper remain unchanged. The six
arm joints solve against factory FK with the mast fixed, within measured joint
limits, a same-side hand workspace and a 72-degree per-joint excursion.
A segment extends the hand at most 30 cm toward the target and stops 20 cm short
of its position (farther away if limited by the arm workspace). Wrist
orientation is free: the IK objective is hand position. The tracked point is the phone camera;
the hand reference is the factory EEF, so these are modeled—not measured surface
clearance—distances. This does not claim full-body or room collision checking.

Commands are bounded to 0.04 motor turns/s and modeled hand translation 20 cm/s.
Targets outside the envelope or without a valid sampled IK path are rejected.
The gesture keeps its chosen direction instead of cancelling for small phone
movements. Brief network jitter is tolerated within the two-second phone age
budget; proximity still stops extension at 20 cm. It holds indefinitely,
records measured joint/hand travel and replans extension/re-aiming in the
background while continuing motor updates. Checked return paths are compacted
where possible so repeated adjustments do not accumulate motion history. Expired
inputs or lost tracking also return along the checked path. A local
feedback/ownership fault releases control instead
of attempting blind recovery. `state/phone-reach.json` records planning, motion,
return and rejection reasons. Ordinary-priority writers yield to manual control.
Both execution flags must be explicitly supplied on launcher restarts.

### Phone joystick driving

Off by default and off again on every launcher restart. Enable it by naming the
one phone allowed to drive:

```sh
python3 scripts/run-bracketbot-runner.py --restart --home-connection <profile> \
  --execute-phone-drive --drive-peer <phone peer ID>
```

The phone's Drive tab posts normalized stick values to Home's scene-scoped
`home-phone-drive` relay. Home applies no speeds and moves nothing; it hands each
robot only its own latest command with an age measured on Home's clock. With
`SPATIAL_DRIVE_PEER` set by the launcher, the runner caches that one phone's
command in `state/drive-command.json` every 50 ms. `drive-bracketbot.py` turns it
into a `drive.ctrl` twist and reports to `state/phone-drive.json`.

Speeds are teleop's: 0.20 m/s forward and a 0.15 m/s wheel difference for turns,
never above 0.28 m/s on a wheel, all halved unless `--drive-fast`. The robot stops
when the stick is released (the phone sends explicit stops), when Home withholds a
command older than one second, when the summed input age exceeds one second, when Home
is unreachable, on low battery, and finally by the base daemon's own 100 ms
`drive.ctrl` timeout. The controller holds `drive.ctrl` only while a command is
fresh and does not start if another program is driving, so manual teleop is not
locked out. A drive controller that exits is not restarted.

### Change the reaching target without rebuilding

With the reach controller running, select a fixed note by its exact display name:

```sh
python3 scripts/select-bracketbot-reach-note.py 'this is dan'
```

This resolves the unique note in the selected Home scene and atomically replaces
`state/reach-target.json` over the operator's SSH connection. The command checks
the robot's configured Home owner/frame before accepting the target. Missing or
ambiguous names are errors. Neither compilation nor runner restart is involved.
Use `--preview` to resolve without commanding a reach.

A note request overrides phone targeting. The nearer hand reaches and stays
extended, adjusting as the robot's scene-relative position changes. There is no
completion timeout or consumed request ID: the target remains active until
replaced or the controller is stopped. `state/note-reach.json` records the
request ID, measured extension and any replan limitation. A new target during
motion cancels the old reach, returns, then selects the new one.
The fixed note has no phone-feed freshness dependency; robot localization,
feedback and joint limits still apply. Target coordinates remain scene-framed,
and the robot continues to operate in Bracketbot space.

Verified on 0184 on 2026-10-11: all 13 capture/stance unit checks passed locally
and on the robot. Home received both wrist feeds. A 30-second physical search
cycle reached the target within 0.66° (left) / 0.72° (right), then timed out and
returned each arm within one encoder tick (0.000244 turns) of its saved pose.
All stance control writers were released afterward. Only the existing Home
process on port 4329 was used. Actual calibration-triggered return remains
unverified: BBOS reports `waiting_for_odometry`, no Portal estimate has been
accepted, and wrist optics/extrinsics still need calibration.

### Handoff status — 2026-10-11

The previous paragraph describes the earlier wrist-search check, not a successful
continuous reach. Later, portal localization did succeed and the note controller
entered `reaching`, but tracking loss cancelled the movement after roughly three
seconds. Sustained reaching and measured target extension remain unverified.

- Robot: Bracketbot 0184. Runtime root:
  `/home/bracketbot/deployments/domain-harness/runner`; scripts in `source/scripts`,
  reports in `state`. Home was running at `http://127.0.0.1:4329`.
- Persistent selected target: `this is dan`, note ID
  `40f71b61-bdef-42d9-b360-e48eea01309d`. Selection is in `state/reach-target.json`.
  The current launch enables reaching but not wrist-search motion. Genuine
  hardware faults remain latched until explicit rearming.
- A low-battery fault was followed by a battery swap; fresh voltage was 20.30 V
  against the installed BBOS 17.8 V cutoff. Do not infer current battery health
  from this historical measurement.
- Home previously retained a pre-reboot scene indefinitely. The scene monitor
  now times out silent streams after 10 seconds and bounds unsubscribe to two
  seconds before rediscovery. Live Home receipt was verified at 298–302 scans,
  approximately 6,500 voxels, and sub-second receive age. Later tracking loss
  occurred while Home remained connected; these are separate failures.
- BBOS intermittently stops producing valid poses. `/dev/shm/slam.log` reports
  `pnp unsolved`, failed relocalization consensus, and pose-graph optimization
  `cost nan -> nan ok=0`. The NaN cause is not established. The head image was
  unobstructed. In a 12-second runner-pause test, SLAM remained lost before,
  during, and after the pause; this does not rule out load contributing to loss.
  The runner was resumed, and no SLAM reset was performed during that test.
- The installed SLAM engine is `bbos/daemons/slam/bbslam.so`; its C++ source was
  not found in the installed tree. The repository named in BBOS's README was
  unavailable to the current GitHub account (404). Further engine diagnosis
  needs the matching source/build or BBOS maintainer assistance.
- Read `local-scene.status.json`, `robot-control.json`, `arm-behavior.status.json`,
  and `note-reach.json` with their timestamps/PIDs. Old reports and old log tails
  can survive process restarts; a running child is not proof of arm motion.

The runtime note target can be changed without compilation or runner restart.
No motor, SLAM, or robot state changes are required to review this PR. The robot
and Home were left running at handoff; no claim is made that tracking is healthy.
