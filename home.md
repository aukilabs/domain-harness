# Home Server

## Home scenegraphs (current)

- **Canonical portals** keeps the 13 supplied poses and sizes, with UCOF1J40N7O
  at zero and the original OpenGL axes. It does not ingest sensor geometry.
- **Home live map** runs the mapper on Home, subscribing to each runner's
  `local-odometry-history`, `local-camera-observations`, and `local-lidar-history`
  Products. It independently detects portals and integrates colored depth/LiDAR;
  no runner scenegraph or voxel checkpoint is an input.
- Each **Galbot scenegraph** remains a separately selectable runner-owned map.

Home independently detects any known-size portal present in the canonical
record. Each robot can see a different marker: Home uses that marker's canonical
position and orientation to solve its odometry-to-shared-map transform. SDK
conversion puts canonical references into the mapper's Z-up basis. UCOF1J40N7O
remains the shared origin; display converts back to canonical OpenGL/Y-up axes.
Neither robot is required to see UCOF1J40N7O specifically.

Per-runner capture-time odometry and camera-to-base transforms place every
measurement in the shared frame. Each raw camera Product declares its optical
frame; Home validates that declaration against the capture calibration. The first
Galbot uses its right wrist, and Echo uses its working left wrist. Original clocks, Product references, and
observation sequences remain attached; timestamps from different robots are
never compared. Missing pose brackets are never extrapolated. A runner's
reconnection/relocalization does not reset the other runner's data or the shared map.

One Home writer integrates both streams of localized raw depth/LiDAR observations
into the same 10 cm voxel accumulator. Both robots can contribute color and
free-space evidence to the same cells. Source-local sequence/time watermarks
reject duplicate observations; occupancy uses the latest three accepted
observations per cell in Home arrival order. Runner scenegraphs are not inputs.
A Home restart starts a fresh collaborative session and archives the preceding
snapshot beside `data/home-observations.scene.json`.
The old `data/home-live.voxels.json` remains saved but is not loaded into either
new Home graph. The canonical journal is now `data/home-portals.json`.
Home publishes separate SDK Products `canonical-scene` and `home-live-scene`.
There is one runner peer per physical Galbot and one Home peer.

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
measured robot-space pose. The chosen shared portal remains stable; its latest
observation updates all reference placements. Home's current UCOF1J40N7O origin
is respected directly, with no second origin offset.

The runner publishes all Home references as `home:<shortcode>` anchors alongside
its measured anchors. They are never fed back into QR observations or voxel
mapping. Home renders these references red and measurements blue; an overlapping
red reference is outlined. The robot's voxels, boot origin, and Home's canonical
map are unchanged by this overlay. No portal backend APIs are used.


## Baseline
- Follow [plan.md](plan.md) and the component-based architecture in [Auki SDK PR #421](https://github.com/aukilabs/auki-sdk/pull/421).
- PR integration points to explore: Map Components, retained snapshots, and authorized catalog queries.
- Scope: P2P clusters and scenegraphs only; leave existing portal infrastructure untouched and make no Auki backend calls about portals.
- Auki relay servers may be used for connectivity to peers without an exposed IP address.
- Act as the authority for the canonical scenegraph, including its voxel map.
- Ingest observations from other peers and integrate them into the canonical scenegraph.
- Ingest scenegraphs built by other peers and reconcile them with the canonical scenegraph.
- Store and serve manually supplied Portal sizes with the scenegraph.
- The 13 real Portal sizes are stored in [config/portals.json](config/portals.json) and served by `GET /api/portal-sizes`. Their supplied poses are in [config/domain-portals.json](config/domain-portals.json). The live canonical scenegraph contains only the 13 real Portals in the domain’s OpenGL Y-up frame. Stored real Portal poses remain unchanged, including zero heights. Synthetic Portal A and fixture geometry are confined to explicit demo mode.
- Store and serve the voxel topography and positioned semantic objects; show both in the 3D visualizer.

## Expected components

Target responsibilities. The first local implementation reuses SDK MapComponent, MapAlignmentChecker, and VoxelMapComponent; authenticated canonical-scene publication is implemented; broader merge support remains in progress.

- **Peer ingestion:** receive observations and scenegraphs from both iPhone and Bracketbot through the shared peer runtime, retaining source metadata.
- **Accepted observation publisher (implemented):** distribute the same validated LiDAR and RGB-D inputs to independent mappers, preserving source identities, calibration and capture clocks.
- **Frame alignment:** establish explicit transforms from contributing peers' frames into the canonical scenegraph frame.
- **Canonical mapper:** create and update the canonical scenegraph from both observation inputs and peer-built scenegraphs; reconcile Portals, semantic objects, and topography.
- **Voxel integrator:** integrate aligned phone and robot observations into the canonical raycastable voxel map; handle contributed voxel scenegraphs under the agreed merge rules.
- **Scenegraph storage and publication:** persist canonical state and manually supplied Portal sizes, and serve them to peers and the visualizer.
- **Runner pose reader:** subscribe to the pinned Galbot peer’s odometry and map-alignment Products; validate session/clock/frame contracts and all 13 reference anchors, then compose the pose into Home’s frame.
- **Browser 3D visualizer:** display canonical voxels, Portals, semantic objects, and the live runner base pose with a forward arrow, focus control, and stale/lost state.

## To decide
- Production host/storage choices beyond the current native Rust server and local JSON contribution journal.
- Contribution permissions and behavior while the Home Server or contributing peers are offline.
- Persistence and serving of Portal maps, semantic annotations, and voxel topography contributed by both the Annotator and Bracketbot.
- How incoming observations and scenegraphs are aligned, combined, and deduplicated, including overlapping evidence received through both ingestion paths.
- Scenegraph merge rules for object identity, conflicting updates, and deletions.
- How the browser-based 3D visualizer should present voxels, Portals, and semantic objects, including detector-generated placements.

## First milestone

Implemented a local SDK-backed mapping and visualization slice; see [README.md](README.md) for run instructions and limits.

- Two simulated contributors submit Portal scenegraphs and depth observations through a localhost fixture API.
- The Home Server aligns their frames, builds a canonical voxel map, and persists accepted inputs for replay after restart.
- The browser displays voxels, manually sized Portals, semantic objects, and camera rays. A synthetic book detection is projected onto an occupied voxel surface.
- Replay, invalid-input rejection, restart recovery, and browser interactions are covered by tests.

The Home Server now publishes a stable canonical-scene Buffer Product over authenticated SDK Component protocols, advertises an explicit connection profile, and pushes updated snapshots to readers. Galbot base LiDAR ingestion over P2P is now implemented. Right-wrist Galbot RGB-D now feeds both maps; next: iPhone input and broader source admission. General semantic scenegraph import and independent voxel-map reconciliation still need implementation; current scenegraph ingestion handles SDK Portal snapshots and accompanying observations.

The old illustrative layout remains in [config/synthetic-room.json](config/synthetic-room.json) for explicit `SPATIAL_DEMO=1` tests. Default Home operation contains no synthetic geometry.

Galbot calibration already exists: its running Component pipeline has an
odometry-to-domain alignment from `HRIPLL40GL6`. Consume its alignment and pose
Products with the explicit domain Z-up to Home Y-up conversion; this read-only
subscription is now implemented and verified live. All 13 reference anchors are
checked over P2P against Home’s supplied map before displaying the pose. The live
pose is transient telemetry, separate from the saved map state. Supplying
Home’s manual portal data to the robot’s existing resolver remains future work.
See [runner.md](runner.md).

## Real voxel mapping slice

- Read `lidar.base-history` from the pinned Galbot peer, with the verified fixed sensor extrinsic and the existing map alignment.
- Match scan timestamps to SDK odometry history; skip moving scans and missing/invalid pose brackets.
- Integrate up to 384 measured points per scan, within 0.3–5 m, at at most 1 Hz into 10 cm SDK voxel evidence.
- Persist checkpoints and source watermarks separately from the preserved demo journal; serve the measured map through the canonical scene Product and visualizer.
- Next: sensor-body masking, per-point motion correction, and explicit rebuilding after calibration or Product changes.

## Selecting independently built scenegraphs

Home publishes its accepted, normalized LiDAR and RGB-D inputs as the SDK Product
`accepted-mapping-inputs`. Each input retains the original sensor Product,
sequence, capture clock/time, selected XYZ returns and capture-time map transform.
The local mapper and the independent mapper on Galbot integrate these same inputs.
Home retains the latest 256 inputs in its voxel checkpoint for bounded replay.

The **SCENEGRAPH** selector offers Home scenegraph, Galbot scenegraph and stored
portals only. A remote-scene receiver validates the configured mapper peer,
publication clock, domain frame and all 13 manual anchors. It caches complete
runner snapshots for visualization/export without merging them into Home's map.
Connection status distinguishes a cached remote map from an active subscription.
Both views retain the red metric portals and live robot pose overlay.

Additional components: accepted-scan publisher; remote scene receiver/cache;
scene source selector. The public mapper connection is `runner_scene` in the
Home peer configuration. No portal backend requests are involved.

## Coordinate conversion and measured color

The Galbot mapper now fetches Home's portal snapshot over authenticated P2P and
uses SDK `MapConventionConversion::between_named_frames` / `convert_snapshot`
to create a separate Z-up/right-forward map with the same origin and metre scale.
Home receives its native Z-up snapshot, validates portal agreement after SDK
conversion, and converts portals, voxel centers and semantic positions into
OpenGL coordinates for rendering. Native snapshot exports retain Galbot's frame.

Measured point RGB is retained through selection, transport, independent fusion,
checkpoint replay and rendering. Color uses SDK additive linear RGB evidence;
colorless points remain uncolored. The active base LiDAR advertises XYZ,
reflectivity, tag, time and offset_nsec, with no RGB. The separate RGB-D Component supplies calibrated D405 depth points with measured
sRGB. Home verifies its capture clock/frame and places them using capture-time
odometry, articulated camera extrinsics and the portal alignment. Its accepted
inputs feed both mappers. The visualizer shows both sensor statuses.

RGB-D is live from the separate Galbot mapper peer. The first live verification
found matching measured colors in both independently built scenes. Coverage is
currently sparse: the D405 input is limited to 0.1–0.5 m, and accumulated LiDAR
free-space evidence can outweigh new depth hits. Bounded evidence/temporal decay
is future mapper work; existing evidence has not been reset or overridden.

### Sensor preview
- A sensor selector below the scenegraph selector discovers camera Products from configured peers over authenticated SDK P2P.
- The selected feed appears inside the scene viewer; selecting a different map does not change the sensor.
- Previews poll up to 2 Hz, report stale frames after 3 seconds without a new sequence, and can be closed with × or None. Offline peers remain visibly unavailable.

Observed portal policy: every accepted camera localization replaces the full
anchor pose (translation and rotation) for that shortcode. No first-pose lock
or averaging is applied. Failed detections retain the last accepted pose;
re-observation does not rebase the robot-boot map or move existing voxels.

## Canonical origin at UCOF1J40N7O

Home uses the portal center as (0,0,0), retaining the original OpenGL axes.
The new frame is `domain:b2881524-954d-4979-b715-240ed8f02807:home-origin-UCOF1J40N7O`.
The translation from the supplied domain frame is (+9.2878, 0, -0.2011) metres.
Raw supplied poses remain in config/domain-portals.json and the stored-domain view.
The saved Home voxel lattice is translated without resampling or changing evidence
or colors. Migration creates home-live.before-ucof-origin.voxels.json as a backup.
The independent physical origin of the Galbot scenegraph is unchanged.

## Live display refresh

The viewer fetches scene state every 500 ms with at most one scene request in
flight. Sensor preview fetches wait 100 ms after each completed frame request.
Preview responses expose the original capture timestamp and clock; do not subtract
that device clock directly from Home wall time. The selected camera remains
independent of the scenegraph selector.

The Home live view renders every calibrated entry in `local_mapping.runners`,
using the SDK-converted display pose. Each peer has a named, colored body/forward
arrow, its own position and focus button, and an independent stale indicator.
The shared layer toggle covers all robots; Focus all Galbots frames them together.
Single-runner views retain only their selected robot pose.

### Camera AR overlay

The PiP **AR** checkbox overlays Home's collaborative live voxels, independently
of the scenegraph selector. It starts off. `local-camera-history` remains a
mapping diagnostic feed; AR uses the ordinary camera Products. All eight ordinary
Galbot cameras and their resized previews use the same generic composition:
image Product + thin sensor pose Product + Home collaborative scenegraph.

Each spatial sensor publishes a separate retained SDK `PoseUpdate` Product
(`<sensor>.pose-history`). Its contract declares exact source/destination frames,
clock and session; its timestamp is the measurement timestamp. The destination
`robot.base` is the runner's declared name for vendor `base_link`. No images or
camera calibration are carried in pose Products. Nonspatial scalar sensors do
not invent frames or poses. Missing TF leaves raw sensor publication working.

Camera metadata declares `pose_product_id` and calibration. The full image and
resized preview share one pose Product; previews retain their adjusted K. Home
keeps at most 64 alignment records per peer in RAM and selects the latest
calibration valid at image capture time, so later portal reacquisition is never
applied to an older image. It joins image and pose at exactly the same timestamp
and clock, checks frame and session continuity, queries SDK base odometry with
`pose_at`, then composes into
Home's canonical alignment. No latest-pose substitution or cross-peer joining.

The viewer always overlays Home's collaborative live map. SDK camera projection
supports fisheye, Brown and pinhole images. Galbot wrist inverse-Brown projection
adds the vendor's tangential correction to SDK Brown projection, verified against
installed `pyrealsense2` reference pixels. The browser rasterizes projected voxel
faces with a depth buffer; curved surfaces are approximated by triangles between
corners. Near-plane-intersecting cubes are omitted. Stale frames, missing
transforms, or changed alignment hide the overlay. It does not occlude voxels
against newly observed/unmapped real-world objects.

## Live map capacity

Home's live map keeps at most 65,536 cell histories. If free-space rays fill that
capacity, it retires the oldest histories containing only free observations,
aiming for 57,344 retained cells to leave room for new scans. Retired free space
becomes unknown. Every history containing a hit is retained, including mixed
hit/free histories needed for the three-observation clearing policy; occupied
colors are preserved. If hit histories alone exceed capacity, ingestion reports
an explicit occupied-capacity error rather than deleting surfaces. This retention
works on the existing populated map without a reset. Home process restarts still
archive the previous live snapshot and rebuild from incoming observations; they
do not resume the prior in-memory vote history.

## Red robot occupancy layer

Home independently reads each configured runner's `robot.occupancy-history`
Product. `/api/robot-occupancy` returns transient red body voxels for Home's live
map; `?peer=<id>` targets that runner's boot-frame scene. The join validates owner,
registered ROS body frame, clock, session and capture timestamp, then uses SDK
`pose_at`, pose composition and convention conversion with the alignment valid at
capture time. The entire voxel group is transformed, preserving cube orientation.

The viewer's **Robot occupied volume** checkbox controls the red layer independently
of topography and illustrative robot pose glyphs. Snapshots replace previous cells;
no robot trails are fused into the collaborative map or persisted on disk. Home
keeps at most six snapshots per robot in RAM to join independently arriving
odometry and occupancy, selecting the newest sample with a valid capture-time
pose instead of extrapolating. Repeated
reads do not refresh expiry; samples expire after three seconds. The UI reports
missing links and labels the gripper travel-envelope approximation. Rendering is
available in Home live and individual runner views once the necessary odometry
(and, for Home, canonical portal alignment) is available.

### Text notes

Home-authored notes are persisted next to the canonical journal in `*.notes.json` and included in published canonical/live snapshots under `registries.text_notes` (`spatial.text-notes/v1`). PR 421’s QR scenegraph does not have a text node; this is a typed extension to the existing scene envelope. Each note has a stable ID, text, explicit map position, anchor portal and height. Notes can be added through the localhost, same-origin `POST /api/notes` with `text`, `portal` and `height_m`. The sidebar creation form has been removed; scene and AR note rendering remain available. Height is along Home’s +Y, not the portal’s local normal.

Home live shares the canonical physical origin. The SDK converts note positions into its Z-up convention, and back into OpenGL for the scene viewer. Camera AR uses the same capture-matched transforms and SDK lens projection as voxels; points behind the camera or outside the image are omitted. Notes are readable billboards drawn above geometry (no occlusion), and follow the AR checkbox. They are not inserted into independently originated runner maps. Notes survive Home/live-map restarts.

Robot name notes are generated in Home live snapshots from each configured runner’s calibrated base pose, at 1.8 m along Home’s up axis. They use stable `robot-name:<peer>` IDs with `anchor_robot` capture-clock/timestamp provenance, replace prior positions as the robot moves, and disappear after three seconds without advancing pose evidence or on disconnection. They share the ordinary text-note rendering path in 3D and camera AR; the old near-base name tag is suppressed when a scene note is present. Robot labels use the latest live robot pose; camera projection remains capture-matched, so rapid robot motion can still show temporal skew. User-authored notes such as abc remain persistent and separate from these dynamic labels.

Right-click the Home scene viewer or a calibrated camera image to place a label. Home performs a 50 cm-diameter sphere sweep (25 cm radius, up to 40 m) against occupied collaborative voxels, with exact rounded-edge/corner collision and contact on the original voxel surface. It does not pick the floor helper, robot volume or a runner-owned map. Scene rays must be in Home's canonical or live display frame. Camera pixels use SDK lens unprojection, checked against the same SDK/vendor projection as AR, and the capture-matched camera transform of the displayed image. Picking also works with AR unchecked; missing calibration leaves ordinary video available and explains why placement is unavailable.

Camera geometry handles are bounded to 64 entries and expire after 15 seconds; placement handles are bounded to 128 and expire after five minutes. The small context dialog freezes camera display while naming the computed hit. Cancel/no hit writes nothing. Saving consumes the handle and persists a fixed Home-space annotation with `anchor_surface` provenance (source Home frame, voxel center and beam diameter). It survives map restarts. No sidebar form is restored.

Select an authored annotation in the Home scene viewer and choose **Delete annotation** to remove it from persisted notes and subsequent scene/AR snapshots. This works for portal-, surface-, and phone-authored notes; generated robot/phone presence names are not deletable. The selection panel stays open through live geometry refreshes. Deletion uses the localhost same-origin `DELETE /api/notes/{id}` endpoint and signals a scene update.

### Landmarks and anchors

A **landmark** is a re-identifiable saved position. An **anchor** is a
re-identifiable full pose. Their generic schema does not require them to be
linked. The default Portal mapper creates one anchor and four child landmarks:
`top_left`, `top_right`, `bottom_right`, `bottom_left`, in decoded printed-marker
order. The children store metre positions in the anchor's local frame (+X printed
right, +Y printed up, +Z outward). Moving the anchor therefore moves its corners;
landmarks do not carry an orientation. Other mappers can create standalone
map-frame landmarks or anchors without children.

In Home, open **Maps → Anchors & landmarks** to browse separate counted lists
for the selected scenegraph. Search by ID, type or identity, then select an entry
to focus its location and inspect its frame, type and provenance.
Landmarks show both stored and resolved map positions; only anchors show rotation.
Older scenes without this metadata are marked unavailable and keep the legacy
Portal list. Unplaced generated tags remain in **Tags**, not in the spatial lists.

The pinned SDK has QR-only scenegraph nodes, so these generic records currently
travel in the scene envelope as `registries.landmarks` (`spatial.landmarks/v2`),
alongside the existing QR anchors for compatibility. Canonical, local-runner and
collaborative Home snapshots include this registry. View responses expose it as
`landmarks`, converted to the view's map frame; anchor-local child coordinates
remain unchanged. Older snapshots without the extension are still readable.

Each record has `recognition: { landmark_type, identity }`. A Portal anchor
uses `auki.landmark.portal` and its shortcode. Each corner has one complete
identity, such as `2ZWHV5L6I04/bottom_right`; there is no separate `feature` field.
Scene node IDs are distinct from recognition: a Home reference overlay and a
measured Portal can describe the same physical identity. Types are extensible
strings, with reserved names also provided for ArUco, AprilTag and image features.
`GET /api/landmarks?landmark_type=auki.landmark.portal` queries the canonical Home
registry, retaining anchors so clients can resolve the returned positions.
Rust consumers can use `by_type` and `position_in_map` to prepare correspondences
for PnP; this change does not replace the existing QR pose solver.

Optional `landmark_types` capability lists reference real SDK components by
`{ peer_id, component_id, manifest_hash }`. They are compatibility declarations
in the extension, not new native SDK manifest fields or claims of authentication.
The current integrated Portal detector/mapper publishes typed 2D corner batches
(`spatial.landmark-observations/v2`) as `landmark-observation-history`. Mapped
features retain `detected_by`, `mapped_by`, the detection output/sequence/capture
clock, and the rectified camera observation reference. Imported reference poses
have no invented detector evidence. Source references persist with snapshots,
but the live detection and rectified-image buffers retain only two observations;
these references are not a promise of permanent raw-image storage.

### Generated fiducials

The Home viewer's **Tags** panel generates standard ArUco 4x4/50 and AprilTag
36h11 markers. A marker ID may be chosen explicitly or allocated from the first
unused ID in that family. Its required physical dimension is measured across the
outer black square; the one-cell white quiet zone is additional. The SVG carries
physical millimetre units, and the print view calls out that browser printing
must use 100% / Actual size rather than Fit to page.

After printing, **Set measured size** records the actual black-square dimension
without changing the family or ID. The local same-origin
`PATCH /api/fiducials/{family}/{id}` accepts `side_length_m`; corrections persist,
update scene metadata, and resize subsequent SVG downloads and print views.

Registrations persist next to `SPATIAL_DATA` in `*.fiducials.json` and are
published in scene metadata as `registries.fiducials`
(`spatial.fiducials/v1`). `GET /api/fiducials` lists them and the localhost,
same-origin `POST /api/fiducials` creation boundary validates the family, ID,
uniqueness and a 10–2000 mm black-square size. The generated SVG and print URLs
only resolve for registered IDs. This registry is the metric definition for a
portal-like detector; the current Galbot camera path still detects the existing
QR Portal family, so ArUco/AprilTag observation and placement are a separate
detector integration.
