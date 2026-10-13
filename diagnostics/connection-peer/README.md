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

Frame, clock and numeric sensor calibration definitions are served through the
SDK Registry endpoint. Catalog metadata binds their exact references and static
interpretation to hashed output IDs. Images and detections do not carry repeated
calibration. The detector consumes the exact observation published on the camera
output; it does not open another camera. A latest-only worker drops superseded
frames, limits upscale retries to once per second, and leaves pose/LED handling
independent of decoding.

All peer data uses authenticated SDK libp2p component/registry protocols. The
Python adapter uses local BBOS shared memory and stdout IPC only. SDK account
authentication/discovery uses SDK infrastructure as before. This does not modify
BBOS, start SLAM, command motors or load maps. Dynamic camera mounts are rejected;
they require timestamped extrinsics rather than a held calibration.

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

Leaving the portal's view does not expire the held alignment. Tracking loss,
stalled pose publication, unavailable Home input or changed portal geometry
withdraws it with a `lost` observation; a fresh portal capture is then required.
SLAM writer changes stop the diagnostic, requiring a fresh frame/session on
restart. Camera and alignment work run outside the pose/LED processing loop.

Tests cover source-pixel retry decoding, fisheye rectification against OpenCV,
capture-time interpolation, tracking loss/gaps/reset refusal, composition with a
rotated camera mount, and Home authority/frame/clock mismatches. Hardware
acceptance additionally requires the robot observing a shared portal and Home
consuming the resulting transform and camera. Camera AR is a separate consumer:
do not re-enable an unmigrated legacy pose path to make that checkbox appear.
