# LED, pose and portal diagnostic

This executable is independent of Home and the full robot runner. The default
mode checks authenticated Home connectivity for the LED controller. `--poses`
also exposes BBOS poses. `--portals` implies poses and adds:

- `bbos-head-camera`: one unrectified grayscale left head-eye stream, nominally
  5 Hz, retained as `bbos-head-camera-history` (two frames, at most 4 MiB).
- `bbos-head-extrinsic`: the configured rigid raw-optical-to-base transform,
  represented by a compact held transform in its capture session.
- `bbos-portal-detector`: Arshak's shared native decode / optimized 2× retry,
  retained as `bbos-portal-detections`. Corners are in the original raw image's
  pixels, with the camera observation sequence and capture timestamp.
- `bbos-home-alignment-<target-hash>`: a separate compact held transform from
  this boot's native Bracket world into the selected Home scene frame.

`--poses`, `--portals` and `--points` also expose `arm-pointer/point_at`
(`spatial-harness.point-at/v1`, contract in `src/point_at.rs`). A target names
its frame by full Registry reference and resolves into `bbos-base` only through
this peer's own compact pose Products: the sampled base pose (at most 800 ms
old), the held head extrinsic and, once aligned, the held Home alignment. All
must share one clock and pose session; a lost or stale link refuses the
request. One request at a time; the result reports the base-frame point and the
observations it used.

By default `point_at` does not touch the arms (`executes: false`). A sixth
argument opts in to the local arm adapter, `scripts/diagnostic-arm.py`:

- `--arm-plan`: the adapter plans each request against the live arm (measured
  joint limits, current pose, feedback health) and refuses what it cannot
  reach. It opens no arm writer and never moves anything.
- `--arm-motion`: a planned request then moves, the way Quest teleop
  (`bbapps/quest_teleop`) moves the arm: it eases to BBOS's own `cfg.home`,
  then the hand glides to a pointing pose (fingers on the target) through
  BBOS's `hybrid_ik`, one solve per tick, speed-capped. It holds 3 s, retraces
  the same path back to where it started, then releases its writers to BBOS.
  The whole path is solved and checked (joint limits, joint speed, the clear
  envelope, the final aim) before anything moves.
- With `--arm-motion` and `--navigate` together, a `point_at` from the robot's
  own Home peer whose target is behind the robot, or that the arm refuses as
  unreachable more than 15° off to a side, first turns the robot in place to
  face the target: one BBOS nav goal at the robot's own position with that
  heading, resolved through the published transforms like any goal. Once nav
  reports arrival the target is resolved again from the fresh base pose and
  pointed at with the arm on its original side. The Operable answers at once
  (`executes: true`); a turn or plan that then fails is reported as a
  `rejected` pointing status. Other callers get the old "turn the robot first"
  refusal. Nav turns at its `HEADING_OMEGA` (0.15 rad/s by default). It refuses to take an arm another
  writer holds, and yields on stale feedback, tilt, low battery, temperature or
  current margins, or a following error. Clear the space in front of the robot.

The adapter declares where the arm IK frame (`arm_base`: X forward, Y left,
Z up, on the drive axle) sits in `bbos-base`; this peer publishes it as the
held `bbos-arm-mount` transform, so a target reaches the arm only through it.
That declaration assumes BBOS base's origin is on the floor under the axle;
verify it on each robot before trusting the aim.

Frame, clock and numeric sensor calibration definitionsFrame, clock and numeric sensor calibration definitions are served through the
SDK Registry endpoint. Catalog metadata binds their exact references and static
interpretation to hashed output IDs. Images and detections do not carry repeated
calibration. The detector consumes the exact observation published on the camera
output; it does not open another camera. A latest-only worker drops superseded
frames, limits upscale retries to once per second, and leaves pose/LED handling
independent of decoding.

All peer data uses authenticated SDK libp2p component/registry protocols. The
Python adapter uses local BBOS shared memory and stdout IPC only. SDK account
authentication/discovery uses SDK infrastructure as before. This does not modify
BBOS, start SLAM or load maps, and commands no motors unless `--arm-motion`, `--drive` or `--navigate` is
given (see `point_at` and teleop above). Dynamic camera mounts are rejected;
they require timestamped extrinsics rather than a held calibration.

## Teleop from Home

`--drive` (with a scene-specific Home profile) lets that Home drive the base.
The peer follows the scene's Home drive stream (`local-scene:<id>:drive`, the
same relay a phone joystick uses), keeps only commands for itself whose driver
is the Home peer in its profile, and hands them to `scripts/drive-bracketbot.py`
at full teleop speeds. That helper is fail-closed: a command older than 400 ms,
a centred stick, low battery or another `drive.ctrl` writer is a stop, and it
holds the writer only while commands are fresh. Home's viewer sends commands
from the keyboard (WASD/arrows, Esc stops) through
`POST /api/local-scenes/{id}/drive`, as Home's own peer.

## Navigation from Home

`--navigate` exposes `base-navigator` with `navigate` and `cancel`
(`src/navigate.rs`), callable only by the Home peer in the profile. Home plans
a route on its scene map (`scene-navigation:<id>` `plan`, as its own peer and
shared so viewers draw it) from the robot's live position, and sends the
thinned waypoints in its scene frame. `scripts/diagnostic-nav.py` declares
where the BBOS nav app's frame sits relative to the SLAM world by nav's own
rule (`T_nav_gl.npy` with QR relocalization on, else identity); this peer
publishes that as the held `bbos-nav-frame` transform, resolves the waypoints
through declared transforms only, sets the final heading along the last
segment, and hands nav `clear` / `add_wp` / `start` on `ws://127.0.0.1:8010/ws`.
Nav plans locally, avoids obstacles and writes `drive.ctrl`; `cancel` sends
`stop`. The nav app must already be running on the robot.

## Recovery

The LED controller (`scripts/bracketbot_slam_led_diagnostic.py`) restarts the
peer whenever it exits, after 2, 4, 8, 16, 32 then 60 s (reset after a run of
a minute or more). The peer exits on purpose when BBOS restarts SLAM (a new
writer is a new pose session, so it starts a fresh one) and when an essential
helper (arm, nav or drive) exits, so a restart respawns them all. Camera, depth
and robot-state helpers degrade without stopping the peer.

## Validation and operation

```sh
cargo test --offline --manifest-path diagnostics/connection-peer/Cargo.toml
```

Build an ARM64 Linux release binary before using
`scripts/run-bracketbot-connection-diagnostic.py --portals` with the existing
`--robot 0125`, `--home-connection` and `--binary` arguments. The launcher retains
its guard against running alongside the full robot runner. No deployment is
performed by the tests.

## Alignment and validation

Home's selected mobile-scene Output pins the exact frame and clock definitions;
the matching SDK portal-only Product supplies geometry over libp2p, avoiding
repeated voxel-map transfers. Selection requires one Home-owned portal layer in
the same declared frame and clock, and checks its MapSnapshot axes/units against
the exact Registry definition. The profile pins the Home peer, Product and physical
scene frame. An origin change requires an updated profile; no cached scene or
HTTP fallback is used. `--check-home` performs this read-only authenticated check
and exits without starting camera/pose capture.

The alignment worker undistorts only detected corners using the declared raw-eye
calibration. SDK QR localization estimates camera-to-Home; a bracketed pose from
the SDK pose Product supplies base-to-Bracket at the same capture timestamp.
The result is `Home<-camera * inverse(Bracket<-base * base<-camera)`.
Translation is interpolated linearly and rotation by shortest-arc SLERP, with
a maximum 250 ms bracket. No extrapolation, tracking-loss crossing, unknown
marker size or axis inference is permitted. Multiple visible known portals must
agree within 15 cm / 10 degrees; individual estimates must pass SDK reprojection
quality checks.

Leaving the portal's view does not expire the held alignment. Temporary tracking
loss, stalled pose publication or unavailable Home input pauses consumers without
revoking calibration. Changed Home reference/portal geometry withdraws it with a
`lost` observation; a fresh portal capture is then required.
SLAM writer changes stop the diagnostic, requiring a fresh frame/session on
restart. Camera and alignment work run outside the pose/LED processing loop.

Camera bytes use the [binary camera contract](../../docs/camera-binary.md).
Alignment initialization validates that exact camera Product before registering
its Component. Diagnostic disk logging uses a bounded background writer so slow
storage cannot stall the capture/libp2p event loop.

Tests cover source-pixel retry decoding, fisheye rectification against OpenCV,
capture-time interpolation, tracking loss/gaps/reset refusal, composition with a
rotated camera mount, and Home authority/frame/clock mismatches. Hardware
acceptance additionally requires the robot observing a shared portal and Home
consuming the resulting transform and camera. Camera AR is a separate consumer:
do not re-enable an unmigrated legacy pose path to make that checkbox appear.
# Measured point clouds

`--points` extends `--portals` (and therefore pose publication) with the existing
BBOS `camera.points` measurements. The launcher accepts the same `--points` flag.
It does not start SLAM, enable driving, or create a robot-side voxel map.

- `bbos-depth-points` uses `spatial-harness.depth-points/v2`: compact binary
  float32 XYZ and optional sRGB bytes through the SDK observation codec. Home
  also continues to read Annotator's existing v1 JSON Products.
- `bbos-depth-extrinsic` is a held compact transform from the registered rectified
  depth optical frame into BBOS base. This is distinct from the raw QR camera.
- Capture timestamps retain BBOS's capture clock; no receipt-time substitution.
- The local helper undoes BBOS's calibrated optical-to-base conversion, preserving
  the depth sensor origin for Home's free-space rays. It reads shared memory and
  writes local stdout IPC only. All peer data uses the existing SDK endpoint/libp2p.
- At most 2,048 measured returns per capture, 5 captures/second, one retained
  capture (128 KiB cap), and a latest-only IPC mailbox. Sampling rotates through
  measured points; it never fills holes with invented measurements.
- Publication and local point preparation require both `set_enabled=true` and
  an active SDK Product subscription. The empty Product is discoverable before
  demand. Listing the Catalog or fetching old retained data does not create
  demand. Home's mapper is a subscriber even with its browser hidden. No
  subscribers means the helper waits without copying or encoding point clouds;
  BBOS's depth daemon itself remains unchanged.
- Invalid/dynamic mounts, writer changes, clock jumps and timestamp regressions
  stop depth capture. Missing/lost/ambiguous transforms prevent Home integration.

Home resolves depth optical → base → odometry → Home using the same generic
capture-time transform-chain resolver as registry-backed AR. It preserves held
transform subscriptions, validates every frame/clock/session, and never
extrapolates sampled poses. Home owns voxelization and accumulation.

Validation is local: `tests/diagnostic_depth.rs` exercises the actual publisher
through authenticated SDK discovery/read; `tests/declared_depth.rs` covers phone
and three-edge cloud integration; `tests/test_diagnostic_depth.py` checks the
BBOS adapter without a robot. Deployment and live sensor verification are separate.
See [compact point-cloud contract](../../docs/point-cloud-binary.md) for the
wire layout and coordinated producer/consumer rollout requirements.

# Robot body state

Any pose mode (`--poses`, `--portals`, `--points`) also starts
`scripts/diagnostic-robot-state.py`, which reads BBOS arm states and
`drive.status` from shared memory (no motor commands) and writes local stdout IPC.
It feeds two Products, both declaring the body they belong to in
`spatial-harness.robot-body/v1` metadata (`src/robot_state.rs` in Home):

- `bbos-joints`: URDF joint positions from each arm's own `q2urdf`, 5 Hz.
- `bbos-health`: per-joint motor temperature and current, and the battery, 1 Hz.

The declaration names the body model (`bracketbot`) and pins its root to the
exact `bbos-base` frame reference and `bbos-capture` clock of `bbos-pose`, so a
reader draws the model wherever that pose places the base. Output IDs bind the
declaration. A missing arm or battery leaves the rest running; if the helper
fails, poses, camera and alignment continue without body state.

Home follows these Products from discovered Catalogs (`src/robot_states.rs`,
`GET /api/robot-states`), reporting its own receipt age instead of comparing
clocks. The viewer draws a peer as its URDF model, with live joints and health
badges, when its registry pose's `from_frame` equals a declared base frame; the
turn from the URDF's ROS body axes into the base frame is read from that frame's
declared Registry axes.

`tests/diagnostic_body.rs` runs this module against Home's follower over
authenticated loopback peers.

# Greeting (`--greet`)

`--greet` adds a `greeter` component with a `greet` Operable
(`spatial-harness.greet/v1`, `src/greet.rs`): the robot says "Hi <name>! Nice
to meet you." through BBOS's speaker, where it stands. Nothing moves. The words
come from [piper](https://github.com/rhasspy/piper) (offline text to speech) in
`scripts/diagnostic-greet.py`, played by `play-bracketbot-speech.py`; one
greeting at a time, and names are reduced to letters, digits and a few marks
before they are spoken. The text reaches piper on stdin only.

piper lives in the robot user's `~/tools`, nothing system-wide:

```sh
mkdir -p ~/tools/piper-voices && cd ~/tools
curl -sSLO https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_linux_aarch64.tar.gz
tar -xzf piper_linux_aarch64.tar.gz && rm piper_linux_aarch64.tar.gz
cd piper-voices && for f in en_US-lessac-medium.onnx en_US-lessac-medium.onnx.json; do
  curl -sSLO https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium/$f; done
```

Without it the greeter still mounts but refuses every greeting.

Home's `peer-greeter` component (`greet_peer`, `src/peer_greeter.rs`) uses it:
a robot that offers both `navigate` and `greet` goes to a peer, following the
peer's live pose, re-sending its route when they move more than 0.5 m (or every
8 s) and aiming 0.7 m short of them; within 1 m it cancels navigation and calls
`greet` with the peer's name. Over the Operable a peer may only ask to be greeted
itself; the Home viewer's "Go say hi" asks before the robot moves, and Stop
cancels the job and the robot's navigation. A job gives up after 3 minutes or 15
failures in a row.
