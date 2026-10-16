# Domain Harness

A component-based spatial mapping prototype with a Home server, Bracketbot runners,
and an iPhone Annotator. It uses [Auki SDK PR #425](https://github.com/aukilabs/auki-sdk/pull/425),
pinned to `3342d589605846f5f45c1de5430dd22923555de2`.

Home builds a canonical scenegraph from calibrated peer observations and lets
users inspect either that map or a runner's independent map. The browser displays
measured voxels, manually sized portals, live robot poses, robot body volumes,
camera previews, calibrated AR overlays, persistent spatial text annotations, and
locally registered printable ArUco/AprilTag fiducials.
Home's **Volumes** panel creates semantic boxes: drag an oriented front edge or
enter a center, base elevation, width, depth, height, and Y-axis rotation. Each
volume has a stable identity, required title, and optional description. Select a
box to edit it using circular handles in the Volumes colour: resize from
either side, change height or depth, move on the ground, or rotate a bottom corner
around the opposite corner. Existing-volume handle edits save on release;
Escape cancels the active drag. Shift snaps resizing/moving to 10 cm and rotation
to 15 degrees. During creation, Shift locks the front edge to X or Z; new volumes
start with Peyote's 60 cm depth and 2 m height, and require a title and Save.
**New volume** starts drawing; "Place a default box instead" is the keyboard
alternative. A selected volume opens in the inspector card, where **Edit**
exposes the exact dimensions. This ports the
fixture/volume manipulation UI, not Peyote's shelving, inventory or measurement
features. Select a
local Home scene first; saved volumes are isolated by scene and coordinate frame,
persist across restarts, and can be selected, edited, hidden, or deleted.
The `spatial-harness.semantic-volumes/v1` collection uses meters and radians and
is included as `semantic_volumes` in the scene-specific mobile Scene Product.
Its authoritative HTTP representation is `GET /api/semantic-volumes?scene=local:<id>`;
creation (`POST`), replacement (`PATCH /api/semantic-volumes/<uuid>`), and deletion
(`DELETE` at the same URL) require the selected `scene` query and its current
`frame` in the request body. POST/PATCH also require a `volume`; Home assigns
the identity on creation. These authored regions are not robot occupancy or
measured geometry and do not alter portal calibration.
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
| Home | `src/main.rs` | Canonical mapping, peer subscriptions, local web API and Three.js viewer |
| Bracketbot runner | `src/bin/bracketbot-runner.rs` | One peer/runtime for BBOS sensors, odometry, portal localization, and a local voxel scenegraph |
| Bracketbot Diagnostics | `diagnostics/connection-peer` | Registry-backed camera, pose, detector, alignment, and optional depth Products |
| Scene reader | `src/bin/spatial-runner.rs` | Subscribe to a peer's complete scene snapshots |
| iPhone Annotator | `ios/PortalViewer` + `ios/PortalBridge` | Native Swift/ARKit app with a Rust SDK bridge |

See [the Bracketbot guide](bracketbot-runner.md) and
[Diagnostics](diagnostics/connection-peer/README.md) for their separate entrypoints.
Run only the intended runner on a robot.

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
binds to `0.0.0.0` by default so the UI is reachable on the LAN. Set
`SPATIAL_BIND=127.0.0.1` only for an explicitly localhost-only instance.
For LAN editing, set `SPATIAL_UI_HOSTS` to the server's LAN address and port
(for example `192.168.1.26:4329`; comma-separated for multiple hosts).
Browser writes must still be same-origin. These checks are not user authentication;
run Home on a trusted network. Rebuilds should preserve LAN access by default.

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

Open **Devices → a peer → Traffic with Home** for a movable dashboard with receive/send
speedometers, recent history, and process totals. The gauge maximum is a saved display
range (1 Mbps–10 Gbps), not measured link capacity; values above it remain visible.
Measurements cover only the selected authenticated Home/remote Peer ID pair,
excluding HTTP, authentication, transport overhead, and retransmissions. Counters reset when Home restarts, but the server retains the last 20 launch
summaries beside `SPATIAL_DATA` in `*.traffic-history.json`. Collection runs every
second regardless of panel visibility; checkpoints save every five seconds and
at graceful shutdown. Each launch keeps its last 60 samples per peer pair, receive/send peaks,
and top 10 intervals ranked by combined throughput. The comparison table includes
uptime averages and percentage changes; differing launch durations affect totals.
An unclean exit can lose the final five seconds and is labeled as a last checkpoint.
Unreadable history is preserved and a persistence warning is shown.

Home exports the SDK `peer-network-traffic` Product from its `peer-network-diagnostics`
Component (`spatial-harness.peer-traffic/v1`), with a registered observation clock,
explicit observer/remote identities, counter-session ID, byte totals, I/O error counts,
and opened/active stream counts. Successful writes do not prove remote receipt or
rendering. The process retains up to 256 identity pairs and reports unattributed
stream opens after that bound. No payloads or credentials are recorded.

The panel compares the same peer pair across launches, ranks launches by peak
receive rate, and exports recorded diagnostics as JSON. History starts with this
version; older runs cannot be reconstructed. Closing the panel stops browser
polling only. The read-only `/api/network-traffic` endpoint returns process session
identity, monotonic elapsed milliseconds, cumulative bytes, and diagnostics. Pass
`?peer_id=<remote Peer ID>` to scope the response to Home’s connection with that
peer. No measurements returns `available:false`; aggregate history is never
relabelled as peer traffic. History from before per-peer instrumentation is excluded.

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

For robot builds and deployment, use [bracketbot-runner.md](bracketbot-runner.md).
Building and starting are separate operations; do not restart another operator's
runner as an implicit build step.

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

Sensor measurements keep their capture-time pose, calibration, clock, and color.
No synthetic voxels are injected into live maps.

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
separately. Generated fiducials persist beside the Home journal in
`*.fiducials.json`. See [home.md](home.md) for detailed behavior.

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
geometry tests require NumPy/SciPy.

Known prototype limits include intermittent peer EOF/reconnection gaps affecting
camera AR, camera availability after robot boot that can require a runner restart,
incomplete hardware-independent deployment automation, and unverified
relay/device combinations. This is a development baseline, not a robot control
or production deployment package.

## Project notes

- [Overall plan](plan.md)
- [Home server](home.md)
- [2D occupancy mapper](occupancy-mapper.md)
- [Bracketbot runner](runner.md)
- [iPhone Annotator](annotator.md)
- [iPhone build guide](ios/PortalViewer/README.md)
- [Vendored voxel adapter changes and license](vendor/auki-voxel-map/LOCAL-CHANGES.md)
