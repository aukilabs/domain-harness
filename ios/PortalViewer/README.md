# Home Portal AR viewer

Native iPhone viewer connected to Home over authenticated Auki SDK P2P.
Sign in with an Auki **User** account, scan one known Portal, and view Home's
Portals, live voxels, and annotations in AR. Double-tap a voxel or the Y=0 floor
to add a shared note. On a LiDAR-equipped phone, **Share LiDAR depth** sends
camera-local measurements and separate transforms for Home to integrate.
Scene updates arrive over authenticated Component observation streams.

## Build and run

From the repository root:

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
sh ios/PortalBridge/build.sh
xcodegen generate --spec ios/PortalViewer/project.yml
open ios/PortalViewer/PortalViewer.xcodeproj
```

Choose your signing team and connected iPhone in Xcode, then Run. iOS 17 or later
is required. The simulator slice builds, but AR world tracking requires a phone.
The generated XCFramework is ignored by Git and must be rebuilt after Rust changes.

1. Run Home with P2P enabled. In Home's viewer, download **runner connection**
   (`/api/home-connection`). This is public pairing information, not a scene export
   or credential. Home must be ready before this endpoint is available.
2. AirDrop the connection JSON to the iPhone and save it in Files.
3. In the app, choose **Connect to Home → Import Home connection profile**.
4. Enter an Auki User email and password authorized for the profile's transport
   Domain, then **Sign in and connect**. This build explicitly uses the same Auki
   development API/DDS/DMS environment as `config/home-peer.example.json`.
5. Allow local network and camera access. Initially, use the same LAN as Home and
   verify that the connection profile contains Home's current reachable address.
6. After the Portal count arrives, point at a known printed Portal. One valid
   measurement aligns immediately, followed by a one-second scanning cooldown.
7. Leave **Share LiDAR depth** enabled and aim at nearby surfaces. The app reports
   captured point counts and whether depth reached Home. Walk slowly; Home's live
   voxel scene returns over P2P. Turn the switch off to pause contributions.

No scene snapshot import is needed. The file you pair with pins Home's expected
Peer ID, Domain, routes, scene frame and Product. It is retained on the phone.
Expired routes require downloading and importing a fresh Home connection profile.

## Networking and lifecycle

`ios/PortalBridge` builds a C ABI static XCFramework around `src/mobile.rs`.
That module reuses `network::subscribe_scene`, the same Rust Component consumer
used by `spatial-runner`. It discovers the Product in the authenticated peer's
catalog, subscribes to the latest complete scene and follows revisions. Validation
checks the pinned peer, Domain, schema/SDK, frame, publication clock, monotonic
revision, SDK Portal snapshot and voxel consistency before passing Portal poses and renderable scene content to Swift.
Occupied voxels (including available color), semantic objects, text notes and
runner poses share the same Home-to-AR transform. Voxels use batched meshes;
scene-only revisions preserve alignment. It makes no Portal metadata or size-lookup backend calls.

User credentials are used for SDK authentication and in-memory authority renewal.
The password is cleared from the Swift form immediately after submission and is
not saved. No App secret is embedded. A persistent phone peer identity is stored
in the app's protected Application Support directory and excluded from backups.
The authentication service endpoints are fixed in Rust; a pairing file cannot
redirect credentials to another host.

Transient subscription failures retry twice before showing an error. A stream
idle for 45 seconds is closed and rediscovered to detect silent disconnection and
Home publication restarts. Initial scene receipt has a 30-second deadline; a
connection attempt has a 60-second overall deadline. Reconnection status labels
any retained AR map as cached. Unchanged Portal data does not reset AR alignment.
Changed Portal geometry or frames require scanning again.

Disconnect and app backgrounding cancel consumption and await subscription,
peer and auth-session cleanup. A short iOS background task gives shutdown time
to complete; iOS can still suspend the app when its background allowance expires.
The app requires sign-in again after a background disconnect. SDK startup is
allowed to finish before cleanup so cancellation cannot abandon a half-started
peer. Local relay booking and discovery advertisement are disabled; supplied
remote relay routes can be dialed through the SDK, but relay connectivity still
needs physical-device validation.

## AR alignment

Vision QR corners and intrinsics use the same captured ARFrame as the camera pose.
The manually supplied encoded-square size establishes metric scale. A calibrated
planar homography supplies Portal-to-camera pose, with an optical-to-ARKit axis
conversion. Alignment is:

`AR_from_Home = AR_from_camera * camera_from_Portal * inverse(Home_from_Portal)`

A single valid measurement with normal AR tracking and bounded reprojection
error calibrates immediately. Scanning pauses for one second after calibration,
then a new valid scan can realign the scene. ARKit tracks the phone between scans.
Tracking loss, interruption and the **Realign** button clear alignment.

Only exact stored payloads and explicit `https://r8.hr/<stored anchor ID>` matches
are used. There are no guessed UUIDs, automatic height corrections, or changes to
Home. Stored zero heights remain unchanged. If Home's stored geometry disagrees
with the physical markers, that disagreement will be visible in AR.

## Verification

```sh
cargo test --locked --test mobile --test network
cargo test --locked -p spatial-portal-bridge --lib
sh ios/PortalViewer/test.sh
xcodebuild -project ios/PortalViewer/PortalViewer.xcodeproj -scheme PortalViewer \
  -sdk iphoneos -derivedDataPath /tmp/spatial-portal-build CODE_SIGNING_ALLOWED=NO build
```

Rust networking tests use loopback-only peers and a fresh local signing authority;
they do not contact shared Auki services. They exercise initial/live delivery,
Domain rejection, cancellation, idle refresh and publication restart. ABI tests
cover invalid inputs, terminal status delivery and omission of credentials.
Swift host checks cover metric pose recovery, rotation, alignment composition,
snapshot contracts and equality used to preserve alignment on unchanged updates.

A successful build and loopback tests do not prove deployment connectivity or AR
accuracy. On-device acceptance still requires real User sign-in, Home delivery,
correct placement of multiple Portals, tracking recovery, network interruption,
background shutdown and camera/local-network permission handling.

## Phone annotations

After aligning with a Portal, double-tap the camera view to place an annotation.
The screen ray is transformed into the calibrated Home frame. Its nearest forward
intersection with an occupied voxel or the mathematical Y=0 floor (up to 40 m)
opens a text editor. The cyan marker previews the location. Save sends the ray
and text through the existing authenticated P2P connection; Home computes the
contact, persists the ordinary text note, and republishes its scene. The floor
works without voxel data. A stable UUID prevents duplicate notes on retry.
Home explicitly permits authenticated Domain peers to append these bounded notes;
the operation does not grant editing or deletion of other scene data.

## Phone presence

While aligned and normally tracked, the phone submits its latest camera pose in
Home's frame over authenticated P2P. Home renders the transient name `nils` at
that position. Captures carry explicit frame endpoints and an AR session clock;
older or repeated timestamps do not refresh freshness. Updates use a latest-value
slot rather than a backlog. Tracking loss withdraws the pose, disconnect attempts
a withdrawal, and Home expires it after three seconds without a fresh capture.
Presence is never saved into the authored-notes file.

Live geometry is subscribed independently from Home's `home-live-scene` Product;
`canonical-scene` remains the calibration and authored-note reference. Only a
live map explicitly declaring the canonical UCOF1J40N7O origin is overlaid. Its
Z-up geometry is converted to OpenGL before sharing the Home-to-AR transform.
An unavailable live stream clears the overlay and retries without disconnecting
the reference scene. Annotation saves raycast against Home's fresh live voxels.

## LiDAR depth into Home

Requires an iPhone that supports ARKit `sceneDepth`. The app samples non-smoothed
scene depth at 2 Hz, keeps high-confidence measurements from 0.2–5 m, and sends
at most 2,048 camera-local points per capture. Image intrinsics are scaled to the
depth-buffer resolution; OpenGL camera points retain X-right, Y-up, Z-back axes.
Depth, intrinsics, and camera pose come from the same ARFrame.

Each capture declares a phone AR session, capture timestamp, camera-to-session
pose, and separate Portal-derived session-to-Home alignment with its observation
time and measured Portal pose. The phone does not pre-transform depth into Home.
A bounded SDK raw-depth Product supplies the source identity; a typed authenticated
P2P ingress operation pushes its latest observation to Home. Home validates peer,
frame, clock, Portal reference, alignment consistency, count, and range before
its existing collaborative mapper composes the transforms and integrates rays.
Both sides keep only the newest pending capture rather than an unbounded backlog.
This initial depth path carries no RGB colors.

Sharing pauses when alignment/tracking is lost, the switch is disabled, or the
app disconnects/backgrounds. Home can build a live scene from the phone alone;
Galbot sensor inputs continue through the same mapper. The older phone presence
label currently still publishes a pre-composed Home pose; refactoring that
separate presence path remains deferred as requested.

Additional checks: `cargo test --locked --test mobile_depth`, Swift depth
projection checks in `test.sh`, and physical confirmation of phone contributions
in Home's live scene after installing and scanning a Portal.
