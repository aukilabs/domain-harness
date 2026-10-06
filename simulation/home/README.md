# Home robot playground

Godot 4.5 project. Standalone by default; optional real SDK camera peer.
One unit is one meter; X east, Y up, -Z north. The 16 × 12 m footprint
contains three bedrooms, an ensuite, a shared bathroom, a hallway, and
an open living/kitchen/dining area. Open roof is intentional for overview.

From the repository root, with the workspace-local engine:

```sh
.deps/godot/Godot.app/Contents/MacOS/Godot --path simulation/home
```

Alternatively import `project.godot` in Godot 4.5+ and press F6 with
`home.tscn` open (or F5 to run the project).

- W/S: forward/reverse; A/D: turn; Shift: slow movement.
- Starts in first-person (robot-eye) view. Tab toggles overhead / robot-eye.
- Click in robot-eye view: mouse look; Escape: release pointer.
- R: restore robot and both movable boxes. Drive into boxes to push them.

The robot has an 80 cm diameter, 1.4 m tall cylindrical collision envelope
and a 1.15 m viewpoint. The full-height envelope protects both forward-mounted
sensors and the camera near plane, including when turning against a wall.
Doors are open 1.2 m apertures with lintels; furniture and walls collide.
The scene uses simple procedural furniture, not photorealistic assets.
Box pushing uses horizontal, speed-limited impulses with a 60 N traction cap;
releasing drive stops the push. The smoke test drives into the hallway box
and checks displacement, grounding, and settling after release.

Headless geometry smoke test:

```sh
.deps/godot/Godot.app/Contents/MacOS/Godot --headless --path simulation/home -- --smoke-test
```

Future sensor adapters should receive timestamped measurements from this
world, not export its ground-truth geometry directly into the mapper.

## Camera and LiDAR SDK peer

Build `cargo build --locked --bin simulator-peer --bin spatial-home`.
Create a separate peer settings file (same structure as Home's settings),
using its own identity file, port 4330, the intended domain and
`advertise_discovery: true`. Never reuse Home's private peer identity.
The local development settings are in gitignored `data/simulator-peer.json`.
Credentials remain in the existing credential helper, never in Godot.

Start the peer, then Godot in another terminal:

```sh
python3 scripts/with-home-credentials.py target/debug/simulator-peer data/simulator-peer.json
.deps/godot/Godot.app/Contents/MacOS/Godot --path simulation/home -- --sdk-peer
```

Home discovers the standard SDK catalog, including component
`simulator-rgb-camera` and Product `simulator-camera`. Select that camera
in Home to start capture. It publishes body-mounted 640×480 JPEG images
at up to 10 FPS, with a single retained frame. The simulated camera does
not follow the overview camera or eye pitch and excludes the HUD/labels.

SDK active subscriptions control rendering demand. Home shares one
subscription across requests and expires it after two seconds without
viewing requests. Merely listing the catalog does not capture anything.
The local SDK patch provides per-Product demand counts: a LiDAR subscriber
does not activate the RGB camera. Both Products retain only the latest sample.

Godot connects to a loopback-only TCP bridge at port 4331. The Rust peer
sends byte `1` for a frame; Godot responds with a little-endian u32 JPEG
length, u64 monotonic capture timestamp in nanoseconds, then JPEG bytes.
Only one request is in flight; maximum payload is 1 MiB. This local
adapter is not an SDK network protocol. Other SDK peers see only standard
Catalog and typed observation subscriptions. Closing Godot shuts down
the peer; restart the peer before reconnecting a new Godot session.

The bridge trusts local processes on this development machine. Do not
expose port 4331 on a network. Camera images are shared with authenticated
peers in the configured domain. Robot mapping is independent of Home alignment.

## Forward LiDAR

The `simulator-forward-lidar` component publishes `simulator-lidar` through
the standard SDK Catalog and observation protocol. Its application payload
is `spatial.simulated-lidar-scan/v1` (a range scan, not a voxel map).

- 120° horizontal sweep, 121 columns at 1° spacing.
- 30° vertical sweep, 16 rows at 2° spacing (−15° through +15°).
- 0.1–10 m valid range; 10 Hz maximum sampling.
- Mount: `(0, 0.65, -0.28)` m in robot-body OpenGL axes; −Z forward.
- First physical surface hit, with robot body excluded. No returns behind walls.
- `ranges_m` is row-major; null means no valid return, never a max-range hit.
- Each scan includes a monotonic timestamp and sensor world position/quaternion.
  This pose is perfect simulator ground truth, **not Home calibration**.
- Ideal instantaneous scans: no noise, reflectance, rolling sweep or motion distortion.

Press **L** to toggle green return points in Godot. The overlay is a local
consumer and is enabled initially; hiding it suspends raycasts unless an SDK
reader requests scans. Debug points and room labels are excluded from RGB.
Home lists the LiDAR and mapper components and Products under Devices.

## Robot-local voxel map

`simulator-local-mapper` consumes the LiDAR Product through a standard SDK
local buffer input. The lifetime of this binding counts as LiDAR demand;
it does not activate the camera. At 1 Hz the mapper integrates the latest
scan into 10 cm voxels and publishes `simulator-local-scene`, retaining one
complete scene snapshot. Intermediate scans are coalesced, not queued.

The map uses capture-time simulated odometry relative to the first robot
pose. This is ideal odometry, not SLAM or Home calibration. Measured rays
provide free-space evidence; missing returns do not. Unseen occupied cells
persist, while the most recent three observations determine occupancy.
The SDK map bound is 65,536 cells; all-free history is retired first.
Restarting the peer starts a new local map; there is no disk persistence.
If Godot pauses during capture, the peer waits for that same response instead
of shutting down or retrying a partially read message. The retained map stays
available, but new integration waits for the simulator to resume.

Home discovers scene products by their standard SDK catalog type. Select
`simulator-local-scene` in Maps to fetch its latest retained scene. Listing
maps does not subscribe to sensors, and unavailable publishers are removed
after catalog refresh. The view is read-only and is not merged into Home.

## Robot-local 2D occupancy

The same robot peer also runs the existing `voxel-to-occupancy-2d` component,
bound locally to `simulator-local-scene`. Once the boot frame is established,
the known world floor (Y=0) is translated into that frame. Projection runs
at 1 Hz, with the first meter above this flat floor as the obstacle band,
10 cm samples, and 2 m retrieval cells. No Home-side mapper is involved.

Select `occupancy-2d` under Home's Network maps. Coral is occupied, green
is free, and slate is unknown. A column is free only when all ten vertical
samples are explicitly observed free, so the narrow vertical LiDAR field
of view can leave much of the floor unknown. Missing observations do not
erase accumulated occupancy. Both maps reset when the robot peer restarts.

Local bridge command `2` requests a scan. Reply: little-endian u32 byte
length followed by UTF-8 JSON matching the scan payload (maximum 128 KiB).
Camera and LiDAR requests are serialized, one in flight; excess work is
skipped rather than queued. Local overlay and remote requests reuse fresh
scans, while each remote reply has an advancing timestamp.

Live read-only P2P check (creates a temporary consumer identity, reads three
scans, disconnects, then removes that temporary identity):

```sh
cargo build --locked --example inspect-simulator-lidar
python3 scripts/with-home-credentials.py target/debug/examples/inspect-simulator-lidar data/simulator-peer.json
```
