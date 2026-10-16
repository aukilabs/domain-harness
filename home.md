# Home server

## Peer-independent scene ingestion

Home serves independently configured local scenegraphs and discovers remote
Products through the SDK Catalog. A peer is a peer: camera, pose, depth, and
alignment consumption uses declared Products, full Registry references, and
capture-time transform chains rather than a robot model or a hardcoded peer.

- Preserve each peer's native coordinate frame and capture clock.
- Resolve declared transforms before integrating measurements into a local Home scene.
- Keep remote scenegraphs separate from Home's own accumulated geometry.
- Do not substitute a latest pose or extrapolate when capture-matched history is missing.
- Keep measured colors and observation provenance through voxel integration.
- Keep authored scenes, notes, portals, and volumes independent of transient peer presence.

Peer traffic uses authenticated SDK/libp2p protocols. The browser uses Home's
local HTTP API; it is not a second peer transport.

## Cameras and capture history

Cameras are discovered from the Catalog and consumed on demand. The selected PiP
can remain open independently of the selected scene. AR and note placement use
the displayed image's calibration and capture-time transform into the selected
Home scene. Missing calibration or transforms pause AR without inventing a pose.
Presentation rotation applies equally to the image and overlays, never to the
underlying frame convention.

See [camera binary transport](docs/camera-binary.md),
[Registry-backed transforms](docs/frame-registry-consumer.md), and
[session clocks and history](docs/session-clocks.md) for the contracts.
See [runner.md](runner.md) for supported robot entrypoints.

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

Use **New note** and click the Home scene viewer, or right-click a calibrated camera image, to place a label. Home performs a 50 cm-diameter sphere sweep (25 cm radius, up to 40 m) against occupied collaborative voxels, with exact rounded-edge/corner collision and contact on the original voxel surface. It does not pick the floor helper, robot volume or a runner-owned map. Scene rays must be in Home's canonical or live display frame. Camera pixels use SDK lens unprojection, checked against the same SDK/vendor projection as AR, and the capture-matched camera transform of the displayed image. Picking also works with AR unchecked; missing calibration leaves ordinary video available and explains why placement is unavailable.

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
portal-like detector. QR Portal detection and ArUco/AprilTag observation and
placement are separate detector integrations.
