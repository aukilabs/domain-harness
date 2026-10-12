# Robot Runner (Bracketbot / Galbot)

## Bracketbot 0184

The BBOS adapter, native runner, deployment scripts, and hardware verification
are documented in [bracketbot-runner.md](bracketbot-runner.md). It publishes the
rectified head camera, stereo depth, SLAM odometry, and an independent local map
through the same single-peer runtime used by Galbot.

## Single Galbot runner (current)

`galbot-runner` owns one authenticated peer and one SDK component catalog. It
publishes the original Galbot sensor adapters (eight cameras and previews,
depth, LiDAR, IMU, joints, force, ultrasonic range and battery) together with
its robot-boot scenegraph. Read-only camera acquisition workers have no peer
identity. The mapper is an in-process component, not another runner.

Start through `scripts/run-galbot-runner.py` using the existing credential
wrapper. The old mapper launcher redirects to this command. Deployment lives
under the original Galbot project's `single_runner` directory. Home discovers
sensors and scenegraphs from the same peer; the old runner-peer target is
removed. Old maps remain on disk. Restart begins a fresh robot-boot map session.

Earlier implementation notes below describe historical stages, not additional
processes to run alongside this runner.



## Second Galbot: Echo (.87)

Echo runs the same single-peer runner at `192.168.9.87` under
`/home/galbot/auki/spatial-harness-runner`. Its independent identity is
`state/runner.identity`; it is never copied from the first robot. Its public
peer is `12D3KooWP7MFxucx3sPDvdYzXU8vBA1oRTmmx1ARGPxTRHQQYEu4`, TCP 4321.
The portable installation includes `scripts/galbot_sensor_names.py` and a local
`runtime/auki_registry` binding, avoiding the first robot's legacy application
imports or global Python changes. Build checksums are in `build-manifest.json`.

Launch from this Mac with:

```sh
python3 scripts/with-home-credentials.py python3 scripts/run-galbot-runner.py --host 192.168.9.87
```

The launcher accepts `--root` and `--binary` overrides; without `--host`, it
retains the first robot's existing installation. Credentials travel over SSH
stdin and are not saved on Echo. Home keeps the first `runner_scene` and adds
Echo under `runner_scenes`; both scenegraphs and camera feeds are selectable.
Each robot's boot frame remains independent. Shared Home fusion requires each
robot's own measured alignment through a known canonical portal.

Echo uses its left wrist D405 for mapping because the SDK currently returns no
right-wrist RGB or depth calibration/data. The launcher defaults `.87` to
`--mapping-camera left_arm`; `.95` retains `right_arm`. It declares the matching
optical frame to the Rust mapper. The measured left calibration, identity
depth-to-color extrinsic, synchronized RGB/depth timestamps, and capture-time
camera-to-base transform were verified. The left view detects `HRIPLL40GL6`.
Live verification confirmed both robots connected and contributing to Home's
shared map: Echo calibrated through `HRIPLL40GL6`, the first Galbot through
`UCOF1J40N7O`. Echo's independent scenegraph also contains measured colored
voxels. Uncalibrated geometry is not placed into Home's shared frame.

## Matched deployments

At the standardization check, both Galbots ran executable SHA-256
`7421c487ebdd0926bd54205aff6efff11308b16352f93b6dcb8a50712b171af6`
from `bin/galbot-runner` under their installation roots, with identical capture
adapters. Later feature deployments have individual `build-manifest.json` records. `.95` selects the right wrist and Echo selects the left. The launcher
uses each installation's binary rather than the shared Cargo build output.

RGB-D capture anchors the SDK's nearest-timestamp lookup on RGB, since Echo's
depth can arrive before its matching RGB image. Exact RGB/depth timestamp equality
and the measured capture-time camera transform remain mandatory. Read-only probes
found 30/30 exact pairs with RGB anchoring on each robot; depth anchoring on Echo
matched 27/30. Both robots passed projection tests; synchronization regressions
cover exact matches and rejection of nearest-but-mismatched frames.

The standardization restart archives prior maps and adapters in each root's
`backups/before-standardize-*` directory; peer identities stay unchanged.
After restart, the same 30-second window produced 57 accepted observations on
`.95` and 56 on Echo; ten subsequent status samples per robot showed no timestamp
mismatch. A subsequent ten-frame probe still rejected one Echo RGB-D mismatch; RGB anchoring
improves availability but does not remove the need for strict validation.
The same ten-scan LiDAR probe saw 19,968 raw points per scan on `.95` versus
86,400 on Echo. After identical fixed-stride/range filtering both retained about
250–260 points per scan, but covered 1,704 versus 339 distinct 10 cm sensor-space
cells. Driver logs identify different hardware: `.95` uses a Livox MID-360,
whereas Echo uses a RoboSense Airy. Both report 10 Hz scans. The LiDAR decoder
now hashes each observation's capture timestamp to vary the starting position
of its sampling stride, retaining the same maximum of 384 points per scan and
0.3–5 m range filter. Local and Home LiDAR decoding use the same timestamp;
replay selects the same measured points and preserves their matching colors.
This avoids repeatedly choosing the same portion of Echo's structured scan.
A read-only ten-scan experiment reached 570 distinct 10 cm cells with fixed
sampling on Echo and 1,780 when shifting sampling positions, with the same point
budget. These are sensor-space coverage counts, not final map voxel counts.
New sessions start fresh robot boot frames. Different viewpoints, sensor returns,
and observation history still produce different voxel maps.

## Live latency

The one runner uses separate in-process workers for sensor publication, mapping,
and checkpoint writes, sharing its existing peer, runtime, and endpoint. Input
mailboxes replace pending camera/depth/sensor samples with the newest per source;
up to 600 odometry samples are preserved for capture-time interpolation.
Registration precedes publication. Mapping publishes at most every 250 ms, and
a separate disk worker saves the latest completed scene without queuing old ones.
`local-scene.status.json` includes sensor publication timestamps and coalesced
sample counts. Sensor history contains published samples; intermediate samples
may be intentionally skipped under load.

## Live voxel updates

Galbot retains the latest three valid observations of each voxel. SDK rays mark
traversed cells free and endpoints occupied; each capture contributes at most one
vote per cell, with hits taking priority over crossing rays in that capture.
Two free observations clear a previously stable occupied cell. Unseen or occluded
cells are retained, and invalid/no-return measurements do not claim free space.
Colors come from the recent occupied observations, so removed objects do not tint
new objects indefinitely. This single-writer policy publishes complete SDK
checkpoints; SDK additive evidence semantics are unchanged. Boot still starts a
fresh map and archives the previous scene.

## Current local-map behavior (2026-10-03)

The single Galbot runner starts mapping at the first valid robot odometry pose.
That robot-boot origin stays fixed. Blue portals are measured with SDK QR
localization, capture-time camera extrinsics and interpolated odometry; every
accepted observation replaces that portal's full pose.

After at least one portal has been observed, the runner reads
`state/home-connection.json` (override: `SPATIAL_HOME_CONNECTION`) and fetches
Home's canonical scene over the authenticated SDK peer protocol. It retries or
refreshes every 30 seconds without blocking sensors. A shared portal establishes
alignment: SDK `MapConventionConversion` expresses Home's portal poses in Z-up
axes, then a full rigid transform maps the shared reference pose onto the
measured robot-space pose. The most recently observed portal known to Home becomes the alignment anchor;
its latest observation updates all reference placements. Home's current UCOF1J40N7O origin
is respected directly, with no second origin offset.

The runner publishes all Home references as `home:<shortcode>` anchors alongside
its measured anchors. They are never fed back into QR observations or voxel
mapping. Home renders these references red and measurements blue; an overlapping
red reference is outlined. The robot's voxels, boot origin, and Home's canonical
map are unchanged by this overlay. No portal backend APIs are used.




Robot peer that consumes data from the other peers, uses semantic annotations as spatial context, and contributes sensor observations to the shared voxel map.

## Baseline
- Follow [plan.md](plan.md) and the component-based architecture in [Auki SDK PR #421](https://github.com/aukilabs/auki-sdk/pull/421).
- PR integration points to explore: map discovery from cluster peers, explicit frame alignment, and localization observations.
- Scope: P2P clusters and scenegraphs only; leave existing portal infrastructure untouched and make no Auki backend calls about portals.
- Auki relay servers may be used for connectivity to peers without an exposed IP address.
- Use manually supplied Portal sizes from the scenegraph; no backend size lookup or fallback.
- Consume spatial context that includes voxel topography and semantic objects positioned from detector observations, such as books on a table.
- Contribute voxel-map observations from robot sensors in the common scenegraph frame, alongside the Annotator's contributions, with capture-time sensor poses and source identity.
- Send contributions to the Home Server, which builds and serves the canonical scenegraph and accepts both observations and peer-built scenegraphs.

## Expected components

Proposed responsibilities; SDK reuse and component boundaries remain to be verified.

- **Robot sensor source:** publish available mapping observations, calibration, capture-time sensor poses, and source metadata.
- **RGB-D source (implemented):** read synchronized D405 depth/RGB with the Galbot SDK, validate calibration and capture-time camera poses, and publish colored depth observations through a retained SDK Product.
- **Localization / frame alignment:** reuse the Galbot's existing QR detector, QR localizer, capture-time camera extrinsics, odometry history and odometry-to-map alignment Components. Bracketbot's sensor-specific adapter is still to be chosen.
- **Scenegraph consumer:** receive canonical voxel and semantic data from the Home Server and expose it as robot context.
- **Observation publisher:** send robot mapping contributions to the Home Server through the shared peer runtime.
- **Local mapper (implemented):** independently acquire and integrate LiDAR/RGB-D with capture-time odometry in a fixed robot-boot frame; publish measured blue portals, voxels, colors, and the local robot pose for Home to select and convert.
- **Robot action adapter (conditional):** connect spatial context to navigation or other actions once the first demo behavior is chosen.

## To decide
- Bracketbot hardware, sensors, and existing software stack.
- Sensor inputs for voxel mapping and whether the Runner contributes observations, locally built scenegraphs, or both to the Home Server.
- First visible behavior: receive/interpret an annotation, navigate to it, or another action.
- Localization, coordinate frames, and connection to robot control.

## First milestone

Implemented a read-only `spatial-runner` client: pair with a public Home connection
profile, authenticate as a distinct peer, discover the canonical scene Product,
and receive full snapshots and subsequent snapshots. `--once` saves one snapshot;
continuous mode follows until interrupted or transport failure. No motion adapter
or implicit robot-to-map alignment is included.

The existing Galbot at `192.168.9.95` runs a Component bridge pinned to SDK
`3342d589605846f5f45c1de5430dd22923555de2`, in transport Domain
`b2881524-954d-4979-b715-240ed8f02807`. Its existing peer is
`12D3KooWLPqS3TQQyJaF9VuCn87wkKRCRXhx2YFNoHy2R1U4iRPd`.
A separate reader lives under `/home/galbot/auki/spatial-harness-reader`; it has
its own peer identity and does not modify the running robot process. Its scene
cache is data for inspection, not authorization to move. See [README.md](README.md).

## Verified Galbot calibration

On 2026-10-02 the existing Component pipeline reports `mapping`, with 13 reference
portals and a calibration from the right-arm camera observing `HRIPLL40GL6`.
It already publishes `robot-odometry-history` and `odometry-map-alignment`, with
explicit frame endpoints, session identity, capture clock and interpolation provenance.
The legacy HTTP `calibrated` flag is a different state path and did not reflect
this Component alignment; use the Component status and Products for this integration.

The destination is `domain.b2881524-954d-4979-b715-240ed8f02807.zup`.
The adapter explicitly constructs it from domain OpenGL coordinates as
`(x, y, z) -> (x, -z, y)`. Its inverse into Home's Y-up frame is
`(x, y, z) -> (x, z, -y)`. Compose this rigid rotation with the published poses;
retain the actual odometry session and capture clock rather than relabeling a pose.

Home now consumes the existing alignment/pose Products through the authenticated
Component protocols, pinning the running Galbot’s peer ID. It verifies all 13
reference anchors against the supplied poses and sizes, preserves the capture
clock/session, and displays the converted robot base position and orientation.
New-only odometry subscriptions avoid presenting retained history as live; a
three-second receive timeout marks the last pose stale, with reconnection after
15 seconds. Lost tracking or invalid contracts hide the marker.

Next: bind canonical scene consumption inside the existing robot runtime. The current robot adapter seeds
its reference map and resolves sizes through legacy domain clients; the Home
integration must instead inject Home's supplied reference anchors/manual sizes
and resolve detected QR payloads locally, without portal backend lookups.
Authorized robot observation ingestion remains separate work.

## Home LiDAR consumption

Home now reads the existing `lidar.base-history` Product over authenticated P2P.
A short read-only vendor query verified `lidar_base_link -> base_link`: position
`[0.22, 0, 0.23]` m and xyzw rotation `[0, 0, -0.70710678, 0.70710678]`.
The live runner was not restarted or modified. Home combines that fixed mounting
transform with capture-time odometry and the verified map alignment.

This first mapping pass accepts stationary scans only, at at most 1 Hz and 384
returns per scan. The separate RGB-D Component described below now adds
calibrated right-wrist depth with capture-time moving-arm extrinsics.
The vendor per-point LiDAR timing fields need declared semantics before deskewing.

## Independent Galbot mapper

`spatial-mapper` runs as a separate component process on Galbot, under
`/home/galbot/auki/spatial-harness-mapper`, with its own persistent peer identity.
It consumes Home's accepted-scan Product, builds a separate SDK voxel scenegraph
locally, checkpoints it on the robot and publishes `canonical-scene` back to Home.
It does not import Home's accumulated voxel geometry. The active robot runner
and its existing sensor/pose components continue running unchanged.

Components: authenticated input subscriber; SDK portal voxel mapper/accumulator;
manual portal reference; checkpoint and input cursor; scene Product publisher.
Source peer, sequence, calibration and capture-time transform remain attached to
inputs. Duplicate input sequence numbers are skipped; missing continuation is reported
instead of silently omitting observations. Reconnection replays the bounded
retained window. A gap larger than 256 accepted scans needs an explicit rebuild
or a future durable observation archive.

This first version depends on Home for scan selection and normalization. Its new
map begins with available retained inputs; historical Home scans predating input
retention are not reconstructed. Embedding the mapper into the existing runner
and allowing it to map while Home is offline remain future work.

## Native Z-up map and color

The independent mapper fetches Home's current complete Portal snapshot before
opening its map. SDK convention conversion imports it into
`domain:b2881524-954d-4979-b715-240ed8f02807:galbot-zup`, named
`ZUpRightForward` (X right, Y forward, Z up), metres, same physical origin.
Sensor-local points are unchanged; SDK target-side transform conversion retargets
the capture-time sensor pose before voxel integration. Native publications and
checkpoints declare this frame. Home performs its own SDK conversion to render.

Existing OpenGL checkpoints migrate all occupancy, free-space and color evidence
by converting voxel centers with SDK geometry and reindexing the same 10 cm
lattice. Counts/cursors survive; `map.opengl-backup.json` preserves the original.
The migrated map is persisted before new input integration. A mismatched source
reference, coordinate declaration or checkpoint owner is rejected.

Both mappers now preserve supplied point colors as SDK additive linear RGB evidence.
The current LiDAR stream has no RGB; its reflectivity is not treated as surface
color. The separate RGB-D Component now supplies color from synchronized wrist-camera
depth and NV12 RGB using validated D405 calibration, RealSense deprojection and
exact capture-time camera-to-base transforms. It publishes on the independent
mapper’s peer without changing the original runner. Both maps receive the same
Home-accepted samples. Capture is bounded to 384 near-field points at up to 1 Hz
and rejects invalid depth, depth edges, unmatched timestamps and missing poses.

Observed portal policy: every accepted camera localization replaces the full
anchor pose (translation and rotation) for that shortcode. No first-pose lock
or averaging is applied. Failed detections retain the last accepted pose;
re-observation does not rebase the robot-boot map or move existing voxels.

Sensor pose Products: the single runner now exposes each spatial sensor's
capture-time transform as a separate SDK `PoseUpdate` Product named
`<sensor>.pose-history`. Source/destination frames, clock and session are explicit.
`robot.base` names the vendor `base_link`. Camera calibration stays on the image
Product; original and resized images declare the same `pose_product_id` and share
one physical sensor pose history. All eight cameras use this path. Missing TF
never blocks raw measurements; nonspatial scalar sensors do not invent a pose.
Home AR composes camera + pose + odometry + canonical alignment + scenegraph.

## Portal reacquisition after manual movement

Home refreshes its odometry-to-canonical transform from each newer accepted known
portal observation. A new portal can therefore correct the robot pose even when
a manual push did not register in wheel odometry. The canonical map frame and
already integrated geometry stay fixed; the correction applies to future poses
and measurements. Delayed images cannot roll calibration back.

The independent robot map still follows its own boot-frame odometry. Its Home
reference overlay switches to the latest observed known portal, but it cannot
retroactively repair geometry captured during unmeasured movement.

Live reacquisition verification: Echo uses `90221M8VMIC` in both Home canonical
calibration and its own reference overlay; shared contributions increased after
deployment. Both running binaries are SHA-256
`eb03f7cd6838c149d6f540c71932015d0687d62bc54c38e8c92b3808e5eb029b`.
The final adapters include native sensor pose Products, bounded dense pose
history, and nonblocking capture-time TF queries.

## Robot occupied volume

The existing single runner also publishes `robot.occupancy-history`
(`spatial.robot-occupancy/v1`) at **2 Hz**. It is a complete replacement snapshot
of 5 cm occupied cells in the registered ROS `robot.base` frame, on the runner's
measurement clock/session. It is never accumulated into the environment map.
History retains at most two samples / 4 MiB. Geometry is loaded from each robot's
installed calibrated `galbot_g1_cali_user.urdf`; the model digest covers the URDF
and collision mesh bytes. Link geometry includes collision origin and mesh scale.

`scripts/robot_occupancy.py` conservatively fills collision hull interiors and
boundary cells using common-time vendor transforms (200 ms behind latest odometry,
nonblocking lookup). It publishes separate native SDK PoseUpdate products named
`robot.link.<link>.pose-history`. Missing transforms omit those parts in the next
snapshot and populate `missing_links`, rather than freezing their old occupancy.
The gripper SDK currently supplies no named joint angles. Both grippers therefore
use explicitly declared conservative travel envelopes over the URDF joint limits,
attached to measured end-effector mount poses. These are approximate volumes, not
measured finger configurations. No gripper or motion commands are sent.

The full model has 41 collision links (25 body links and 16 gripper links).
The installed user URDF can omit attached tools: Echo currently exposes 25 body
collision links. Product coverage and model hash describe the actual loaded model;
no tool geometry is invented when absent. The initial .95 probe covered all 41.

Link rasterizations are reused while a conservative vertex-motion bound remains
within 2 mm; their hulls include that margin, and larger articulation recomputes
the affected part. Live cached updates measured roughly 20–85 ms and sustained
2 Hz on both robots. Numerical workers use one BLAS thread to limit contention.
Echo uses SciPy 1.10.1 in its private `runtime/occupancy` directory because its
system SciPy is incompatible with its installed NumPy. Dependency failures are
contained in the occupancy worker and do not stop camera/odometry acquisition.

Deployment verified 2026-10-03: both single runners use binary SHA-256
`2a3b130d32a4ed31a0ff7f960be284abc1fb9c4db70ad3878f2c0658534d13f1`.
Final adapter and rollback paths are recorded in each installation's
`build-manifest.json`. Home displayed both occupied volumes concurrently:
.95 had 1,859 cells / 41 collision links; Echo had 1,805 cells / 25 collision
links in its currently installed body-only URDF. Both measured publication
counters advanced four samples over two seconds. Geometry, timestamp joins,
transport replacement/expiry and viewer behavior passed their checks.

## Semantic volume speech (Bracketbot)

### Bracketbot pose-graph correction handling

Bracket uses capture-time geometry: ordinary BBOS pose-graph updates are accepted
as live poses in the same robot frame. Capture never reads optimized trajectory
history, suppresses odometry for PGO, or back-propagates corrections into old
points or portal measurements. Geometry may drift or have discontinuities; this
is intentional. No raw depth is retained for correction replay on Bracket.

Portal localization produces a separate robot-to-Home relationship. Home retains
the last validated relationship per authenticated producer, native frame and
capture clock/session, and applies it to new native observations. Neither mapper
revisits old samples. Robot presence always carries a native base pose; Home
composes it for labels. Expired pose samples do not expire the relationship.

Incoming notes, phone poses and volume collections declare their coordinate
frame. Consumers convert between the named robot and Home frames using the last
known relationship, or reject unknown frames. Missing/stale live poses still
pause actions; this is separate from the validity of a saved frame relationship.

Producer restart, clock regression and a decreasing PGO counter require a new
capture session and fresh calibration. After an ordinary runner restart, show a
known portal once before driving away.

The previous history-replay implementation caused mapping pauses despite healthy
BBOS tracking. A stationary 60-second runner-off comparison reduced median total
CPU usage from 97.8% to 66.0%; restoring the runner raised it to 94.5%. This
establishes overhead, not causation of actual SLAM tracking loss.

The 2026-10-12 fix was built on the laptop with cached Linux ARM64 Docker:
`build-bracketbot-correction.py`, then transferred/compatibility-checked with
`install-bracketbot-correction.py`. The installer refuses to overwrite newer
deployed sources. `restart-bracketbot-presence.py` keeps the existing supervisor
arguments/environment. Home and the BBOS SLAM daemon are not rebuilt/restarted.
Tests verify that corrected calibration survives with no new marker observation
and post-correction depth scans remain available for contributions. Live driving
verification still requires a fresh portal observation after deployment.

### Presence and speech

Robot name labels use a separate authenticated `home-robot-presence:<scene>`
operation, independent of geometry contribution acceptance. The runner supplies
its native `robot.base` pose and mapper-owned Home transform; Home composes them
without rebasing the robot. Presence expires after three seconds and is withdrawn
when tracking/alignment is unavailable. Robot labels are read-only and separate
from phone presence, so robots never become phone-reaching targets. Pose
publication diagnostics are written to `robot-presence-status.json`.

The pose path passes local authenticated integration tests, including publication
without any voxel geometry. The runner-only update was installed on Bracketbot
0184 on 2026-10-12 using `build-bracketbot-presence.py`; unrelated deployed
sources were preserved. `restart-bracketbot-presence.py` restarted its existing
supervisor with identical arguments/environment, retaining phone driving. Home
was not rebuilt or restarted; its receiver must be deployed separately. Current
pose diagnostics report the receiver unavailable and the mapper alignment stale.

The supervisor starts `speak-bracketbot-volumes.py` by default. The runner
subscribes to the authenticated Home mobile scene selected by
`home-connection.json` and writes only its semantic-volume extension to
`semantic-volumes.json`. This currently uses the shared scene Product (including
its geometry on the wire), not a dedicated lightweight volume Product.

The speech controller transforms the live **robot.base** position into Home
space without changing the robot's native frame. It reads each nonblank
description after a 0.5-second entry dwell. Boxes honor yaw and height; overlapping
volumes are announced in stable ID order. A 5 cm exit margin suppresses boundary
chatter. Lost/stale localization pauses announcements and is not treated as an
exit. Editing a description while already inside does not repeat it.

Offline synthesis requires `espeak-ng` (or `espeak`) on the robot. Playback uses
the existing BBOS `speaker.audio` topic and speaker daemon's Python environment,
including its NumPy and soxr dependencies, without claiming the ALSA device. The speech
worker reports backend/playback errors in `volume-speech-status.json` and does
not block SLAM, change audio services, or command motors. Failed announcements
are not retried continuously. Use supervisor `--disable-volume-speech` to opt out.

Verified on Bracketbot 0184 on 2026-10-12: ARM64 build/install, offline synthesis,
BBOS speaker playback smoke test, and authenticated ingestion of the Home
`Office` volume. The launched runner resumed mapping but its Home alignment
remained stale, so a real localized volume-entry announcement remains unverified.
