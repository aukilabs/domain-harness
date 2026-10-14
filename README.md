# Domain Harness

A component-based spatial mapping prototype with a Home server, Galbot runners,
and an iPhone Annotator. It uses [Auki SDK PR #425](https://github.com/aukilabs/auki-sdk/pull/425),
pinned to `3342d589605846f5f45c1de5430dd22923555de2`.

Home builds a canonical scenegraph from calibrated peer observations and lets
users inspect either that map or a runner's independent map. The browser displays
measured voxels, manually sized portals, live robot poses, robot body volumes,
camera previews, calibrated AR overlays, persistent spatial text annotations,
an in-memory Wi-Fi/cellular survey overlay from a walking Android phone, and
locally registered printable ArUco/AprilTag fiducials.
The iPhone app aligns its ARKit session by observing a known portal, displays
Home geometry, places annotations, publishes phone presence, and contributes
high-confidence camera-local LiDAR depth with separate alignment evidence.
See the [iPhone guide](ios/PortalViewer/README.md) for device requirements.

Peer communication uses authenticated SDK Component protocols. Authentication,
discovery, and optional relay connections may use Auki services; portal geometry
and sizes come from local configuration, with no portal backend API calls.

## Components

| Part | Entry point | Role |
| --- | --- | --- |
| Home | `src/main.rs` | Canonical mapping, peer subscriptions, local web API, Three.js viewer, and connectivity survey overlay |
| Galbot runner | `src/bin/galbot-runner.rs` | One peer/runtime for sensors, odometry, portal localization, and a local voxel scenegraph |
| Scene reader | `src/bin/spatial-runner.rs` | Subscribe to a peer's complete scene snapshots |
| iPhone Annotator | `ios/PortalViewer` + `ios/PortalBridge` | Native Swift/ARKit app with a Rust SDK bridge; the same C ABI is linked by the Android connectivity surveyor |
| Connectivity surveyor | sibling [connectivity_surveyor](https://github.com/aukilabs/connectivity_surveyor) repo | Android walker that publishes calibrated Wi-Fi, cellular, and path samples to Home |

The Galbot deployment publishes sensor data; it does not enable robot motion,
navigation, arm, or gripper commands. `runner-integration/original` and `retired`
are reference source, not additional running processes.

## Run Home locally

Requirements: Rust 1.94+, Node.js 20+, npm, and access to the pinned Auki SDK Git
repository. Browser tests use an installed Google Chrome. The iPhone build also
requires macOS, Xcode, and XcodeGen.

```sh
npm ci
cargo run --locked
```

Open [the local viewer](http://127.0.0.1:4319). The default view uses the supplied
portal geometry and has no synthetic voxels. P2P is opt-in; real robot observations
require the configuration below. `SPATIAL_PORT` changes the HTTP port; the server
binds to `127.0.0.1`.

For the separate synthetic demonstration:

```sh
SPATIAL_DEMO=1 cargo run --locked
# In another terminal, with that demo server running:
npm run demo
```

The synthetic room is illustrative. It is not inferred from the portal positions.
Live mode rejects fixture writes.

For a moving fake robot producing voxels and a 1 Hz occupancy map **inside Home's
real UI**, see [the robot simulator](occupancy-mapper.md#moving-robot-in-the-real-home-ui):

```sh
env -u SPATIAL_PEER_CONFIG SPATIAL_SIMULATED_ROBOT=1 SPATIAL_PORT=4323 cargo run --locked --bin spatial-home
```

In Maps, choose **Simulated robot · voxels** or **Simulated robot · occupancy 2D**.
This uses isolated loopback peers; it does not connect to a real robot.

## Connect peers

`config/home-peer.example.json` is a development-lab example, not a portable
production configuration. Replace the Domain, advertised LAN address, runner Peer
IDs, and routes for your installation. The checked-in connection profiles contain
public pairing information only. Every peer needs its own persistent identity.

```sh
cargo build --locked --bins
# Supply AUKI_ACCESS_KEY and AUKI_APP_SECRET through your local environment.
SPATIAL_PEER_CONFIG=config/home-peer.example.json target/debug/spatial-home
```

The config selects App login. For User login, set `login` to `user` and supply
`AUKI_EMAIL` and `AUKI_PASSWORD`. Credentials are not read from a committed file.
The optional `scripts/with-home-credentials.py` helper reads a locally supplied
`data/homeserver.rtf` into environment variables. `data/`, identity files, and
local environment files are ignored by Git.

Home provides a downloadable connection profile at `/api/home-connection` once
its peer is ready. Consumers validate its pinned peer, Domain, Product, and frame.
All authenticated peers in the configured Domain can read exported Products.
Home's phone ingestion operations additionally validate the caller and submitted
spatial contracts; this prototype does not provide per-user application roles.

Example one-shot scene read with your own peer config and downloaded profile:

```sh
cargo run --locked --bin spatial-runner -- \
  --config data/reader.json --home data/home-connection.json \
  --once --output data/received-scene.json
```

The Galbot launcher assumes the vendor SDK is installed on the robot, a built
ARM64 runner and adapters have been deployed, and the robot has its own peer config
and identity. It passes credentials over encrypted SSH stdin. See [runner.md](runner.md)
and [runner-integration/README.md](runner-integration/README.md) for installation
paths, sensor choices, and the existing lab setup.

```sh
python3 scripts/with-home-credentials.py python3 scripts/run-galbot-runner.py \
  --host 192.168.9.95 --mapping-camera right_arm
```

## Connectivity survey overlay

Home accepts compact radio samples at a phone's calibrated pose and draws them
on the live map. This is a dedicated ingest, not a text note. The Android walker
lives in the sibling [connectivity_surveyor](https://github.com/aukilabs/connectivity_surveyor)
repository. That app's local JSONL file is the archive. Home keeps an in-memory
overlay of at most 1024 samples; restarting Home clears the map.

The phone follows Home as an authenticated P2P peer (import
`/api/home-connection`, Auki User sign-in, live portal scene, phone presence).
Once the pose is in Home metres, each survey snapshot is queued through the
portal-bridge C ABI rather than as an annotation.

```
phone JSONL archive
        │
        ▼
spatial_mobile_survey  (≤ 32 KiB UTF-8)
        │
        ▼
MobileClient overlay queue  (512)
        │
        ▼
home-connectivity-survey  operable ingest
        │
        ▼
registries.connectivity_survey
        │
        ▼
scene JSON survey_samples  →  Three.js spheres
```

`POST /api/survey-import` can load the same overlay from phone JSONL or a compact
JSON array. It is a localhost write, the same boundary as note edits. It is not a
substitute for the phone archive.

### Contracts

| Name | Value |
| --- | --- |
| Component | `home-connectivity-survey` |
| Registry schema | `connectivity-survey.samples/v1` |
| Sample datatype | `connectivity-survey.sample/v1` |
| Phone JSONL schema | `connectivity-surveyor.sample.v1` |

Home advertises the component on the authenticated cluster catalog. After
pulling this branch, restart `spatial-home` so the phone can look it up. An older
Home binary answers catalog requests without `home-connectivity-survey` and the
phone reports that the overlay was not accepted.

The compact overlay JSON the phone sends over P2P is:

```json
{
  "id": "sample-uuid",
  "captured_at": "2026-10-14T05:03:00Z",
  "scan_age_ms": 12,
  "translation": [1.5, 1.2, -2.0],
  "wifi": [
    {"ssid": "Auki-Office", "bssid": "aa:bb:cc:dd:ee:ff", "rssi_dbm": -47, "frequency_mhz": 5180}
  ],
  "cellular": {"radio": "NR", "dbm": -89, "level": 3},
  "path": {
    "wifi": {"rtt_ms": 18, "loss_pct": 0, "window": 10, "target": "8.8.8.8", "ssid": "Auki-Office"},
    "cellular": {"rtt_ms": 42, "loss_pct": 10, "window": 10, "target": "8.8.8.8"}
  }
}
```

`translation` is the phone pose in Home metres (OpenGL Y-up), including camera
height. It is not flattened to the floor. Ingest keeps one Wi-Fi row per SSID,
the strongest BSSID of that name at that pose, then sorts by RSSI and truncates
past 256 names. Hidden SSIDs stay empty strings; the viewer labels them
`(hidden)`. The phone list can still show every BSSID; Home filters by name.

Optional `path.wifi` / `path.cellular` hold a rolling probe to `8.8.8.8`
(`rtt_ms`, `loss_pct`, `window`, `target`). Wi-Fi path rows may include `ssid`
for the associated network. Latency and loss in the survey sample box use that
name, not a generic "Wi-Fi" label.

Unknown JSON fields are rejected (`deny_unknown_fields`). Over-long SSIDs are
truncated to 32 bytes; RSSI is clamped to −127…0 dBm.

### Viewer

Enable the **Survey samples** layer. A control appears once at least one sample
is present. The filter lists **Best RSSI**, **Cellular**, **Wi-Fi latency**,
**Wi-Fi loss**, **Cellular latency**, **Cellular loss**, then every unique SSID
in the overlay. The option list is rebuilt only when that SSID set changes, so a
500 ms scene poll does not reset an open picker or fire Chrome's `change` onto
**Best RSSI**.

Sphere colour matches the phone Quality palette (`#3ecf8e` → `#e6c34a` →
`#ef4444`):

| Metric | Good | Mid | Poor |
| --- | --- | --- | --- |
| Wi-Fi RSSI | −50 dBm | −67 dBm | −80 dBm |
| Cellular RSSI | −80 dBm | −100 dBm | −110 dBm |
| Wi-Fi RTT | 20 ms | 50 ms | 150 ms |
| Cellular RTT | 50 ms | 100 ms | 250 ms |
| Loss (either) | 0 % | 2 % | 10 % |

Survey materials set `toneMapped: false` so the scene renderer's ACES filmic
tone mapping does not wash greens toward yellow. **Best RSSI** colours each
sphere by the loudest AP at that pose. Selecting an SSID colours that name's
strongest BSSID, which matches the corresponding row on the phone. Click a
sphere for pose and the named metric (SSID or cellular radio, value, quality
word).

### Phone queue and Stop

The portal-bridge C ABI `spatial_mobile_survey` rejects payloads over 32 KiB.
The phone client keeps a 512-sample mpsc queue and publishes from a spawned
worker so a scene-follow timeout does not cancel in-flight overlay work. Poll
snapshots include `survey_pending` (queued plus the current invoke).

**Stop** on the phone ends capture and flushes JSONL immediately; the peer stays
up until the remaining overlay samples drain (or 30 s). **Disconnect** and
process teardown cancel the peer after the same drain. Backgrounding the Android
activity must not call `spatial_mobile_stop`; that lives in the surveyor app, not
in this repository.

Rebuild the Android static library after this branch with the surveyor's
`scripts/sync-home-bridge.sh` (`DOMAIN_HARNESS` pointing at this checkout), then
rebuild the debug APK.

### Limits

- Home's overlay is not the phone JSONL archive. Restart Home to clear spheres.
- Duplicate sample ids are ignored. Past 1024 accepted samples, the oldest is
  dropped.
- Typical office scans fit in 32 KiB after unique-SSID compaction. Pathological
  scans: the phone drops the weakest SSIDs last; the C ABI still rejects a
  payload that exceeds 32 KiB.
- Import and ingest compact Wi-Fi the same way. They do not reject a normal
  scan for having more than 16 BSSIDs; that cap was leftover from notes-sized
  payloads.

### Tests

```sh
cargo test --locked --lib mobile_survey --test mobile_survey
```

Unit tests cover unique-SSID compaction, RSSI clamp, JSONL import, and path
metrics including wifi `ssid`. The loopback test `sample_reaches_home_over_p2p`
invokes `ingest` across two in-process peers.

## Geometry and persistence

Portal sizes in `config/portals.json` are metres measured across the encoded
square, excluding its border. The supplied domain poses use OpenGL Y-up axes;
all stored portal heights are zero and do not establish actual mounting heights.
SDK coordinate conversion and explicit rigid alignment connect Home, runner,
sensor, and ARKit frames. Raw timestamps retain their producing peer's clock.

Scene metadata distinguishes position-only **landmarks** from full-pose
**anchors**. The default Portal mapper creates an anchor with four named corner
landmarks; parenting remains mapper policy. Typed recognition, component
capabilities and observation provenance travel in `registries.landmarks`.
`GET /api/landmarks` supports filtering by `landmark_type`. This is a versioned
extension alongside the pinned SDK's existing QR scenegraph, not an SDK fork.
See [home.md](home.md#landmarks-and-anchors) for its contracts and frame rules.

Galbot LiDAR mapping samples at most 384 measured points per scan in the 0.3–5 m
range. A timestamp-derived sampling offset avoids repeatedly selecting the same
parts of a structured scan. Capture-time odometry and sensor extrinsics place
measurements; moving LiDAR scans are rejected until motion compensation is
implemented. Wrist RGB-D supplies measured near-field color. No synthetic voxels
are injected into live maps.

Live occupancy uses the latest three observations per cell. At the 65,536-cell
limit, old histories containing only free-space observations can become unknown;
occupied cells, colors, and mixed hit/free histories are retained. Hit histories
alone exceeding capacity still produce an explicit error.

Home's legacy journal/checkpoint store uses `SPATIAL_DATA` (default
`data/home-live.json`). Its collaborative raw-observation map separately uses
`SPATIAL_HOME_MAP_DATA` (default `data/home-observations.scene.json`). That
collaborative map is archived and rebuilt from live input on Home restart; it does
not resume its prior in-memory vote history. Runner restarts likewise start a new
boot frame and archive the previous local scene. Spatial text notes persist
separately. The connectivity survey overlay does not; it is rebuilt only from
live ingest or `POST /api/survey-import` after a restart. Generated fiducials
persist beside the Home journal in `*.fiducials.json`. See [home.md](home.md)
for detailed behavior.

## Validation

```sh
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
npm run test:browser
sh ios/PortalViewer/test.sh
```

Browser tests start an isolated demo server on port 4318 with a temporary journal;
they do not reuse the running Home. Some Rust tests are explicitly ignored unless
a real sensor recording is supplied. Python adapter tests live under `tests/test_*.py`:
geometry tests require NumPy/SciPy, and RealSense projection tests also require
`pyrealsense2` in a compatible Galbot Python environment.

Known prototype limits include intermittent peer EOF/reconnection gaps affecting
camera AR, camera availability after robot boot that can require a runner restart,
incomplete hardware-independent deployment automation, and unverified
relay/device combinations. This is a development baseline, not a robot control
or production deployment package.

## Project notes

- [Overall plan](plan.md)
- [Home server](home.md)
- [Connectivity survey overlay](#connectivity-survey-overlay)
- [2D occupancy mapper](occupancy-mapper.md)
- [Galbot / Bracketbot runner](runner.md)
- [iPhone Annotator](annotator.md)
- [iPhone build guide](ios/PortalViewer/README.md)
- [Vendored voxel adapter changes and license](vendor/auki-voxel-map/LOCAL-CHANGES.md)
