# Binary camera delivery

Camera Products opt into `spatial-harness.video-frame-binary/v1` in their
Camera payload schema. Their datatype is still the SDK's `VideoFrame`. The
named observation codec is `spatial-harness.video-frame-packed/v1`.
Home selects this codec by schema for **any peer**, never by robot name/ID.
Legacy `auki.video-frame/v1` JSON Products remain readable without a phone
rebuild. Upgraded producers require an upgraded consumer; binary data is not
silently sent under a legacy JSON schema.

The SDK retains the exact Output reference, source sequence and capture-time
nanoseconds. Frame/clock references and sensor calibration remain registry-backed.
The existing AR resolver checks the same capture-time transform chain, without
latest-pose substitution, extrapolation or convention changes.

The binary observation is a 16-byte header followed by unchanged image bytes:

- Bytes 0–3: `ACF1`.
- Bytes 4–7 and 8–11: width and height, unsigned 32-bit little endian.
- Byte 12: encoding (`1` JPEG, `2` JPG alias, `3` RGB8, `4` LUMA8).
- Bytes 13–15: reserved zeroes.
- Remaining bytes: image, at most 8 MiB. Dimensions are 1–4096. Raw image
  lengths and JPEG dimensions are validated against the payload and Output.

The diagnostics capture adapter uses a bounded local pipe, not a network
transport. A one-time u32 length + JSON calibration record is followed by
records of u64 capture nanoseconds + u32 JPEG length + raw JPEG bytes. It still
extracts the calibrated grayscale left eye from BBOS stereo imagery at roughly
5 Hz and JPEG quality 95. This change does not alter camera geometry or detector
input. Local QR detection remains a legitimate consumer even with no PiP open.

The diagnostics localizer binds its calibration to the exact binary camera
Product and its registered optical frame/clock. The pinned SDK's convenience
constructor accepts only the legacy JSON schema, so the adapter validates the
binary Output and constructs the localizer's public calibration explicitly. It
does not relabel the Product or change the optics. This validation runs before
registering the alignment Component; the full startup path is regression-tested.

Diagnostics uses a bounded background log writer (64 messages, 4 KiB each).
Filesystem latency must not block the capture/libp2p event loop; a full queue drops
diagnostic messages, not camera/pose data. Identical alignment initialization
errors are logged once until the error changes or initialization succeeds.

Runner-to-Home observations use authenticated SDK/libp2p only. The SDK's small
control envelopes are still JSON; the image body is not JSON or base64.

For browser PiP, Home responds with
`application/vnd.spatial-harness.camera-frame`: u32 little-endian metadata length,
UTF-8 JSON metadata (at most 1 MiB), then raw image bytes. Metadata and image
belong to one exact observation, including AR/picking data. Timestamps and
sequences are strings to preserve integers larger than JavaScript's safe range.
PiP projects only its displayed notes and drawings. It does not expand the Home
voxel map into unused triangle arrays in the camera metadata. Voxels remain in
the scenegraph and available to the 3D viewer and server-side raycasts; omitting
them from this annotation-only response does not discard measured map data.
The browser-to-Home API remains local HTTP; no peer bypass is introduced.
The frontend can still read a legacy Home's JSON response during rollout.

Qwen's optional one-shot vision adapter remains a separate boundary: its local
model API requires a base64 image. That conversion happens only on explicit
visual questions, not in the live camera stream.

Deployment order: upgrade Home, then the camera publisher. Existing phone and
simulator products still use their advertised legacy codec. Building this source
does not upgrade other deployed apps automatically.
