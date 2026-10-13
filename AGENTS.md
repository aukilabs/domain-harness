# Agent guidelines

This repository builds Home, the phone apps and the robot runner on the
[Auki SDK](https://github.com/aukilabs/auki-sdk). The SDK's own `AGENTS.md`
and crate READMEs are the authority; this file carries the parts every change
here must follow. `Cargo.toml` pins the SDK revision in use. Read that
revision, not `develop`: the Component, odometry and scenegraph crates
(`labs/auki-components`, `labs/auki-odometry`, `labs/auki-registry`,
`labs/auki-scenegraph`, `labs/auki-component-protocol`) differ between them.

## Declare, never infer

A reader must be able to use any published value from its declarations alone.

**Frames**

- Every published map declares its reference frame and coordinate convention.
- Every pose or transform carries explicit source and destination frames, on the
  value or in a mandatory enclosing contract such as a `PoseContract`. Never
  infer endpoints from a field name, a peer, Domain membership or a default.
- A frame is a full `FrameRegistryEntry`: owner, ID, axes, handedness, units.
  Attach it to an Output with `ConfiguredObservableSpec::in_registered_frame`.
  Camera Outputs require it.
- A convention is axes, not alignment. Two frames with equal conventions are not
  aligned; alignment needs an explicit transform with both frames and a clock.
- Validate frame compatibility before composing. A missing or mismatched frame
  is an error, not something to relabel.
- Changing a frame definition changes its hash and the Output's identity. After
  an odometry reset, use a new destination frame ID and session ID.

**Publish data in the frame it was measured in.** Do not turn, flip or re-axis
sensor data to suit a viewer and then describe the change in free-form
metadata. A phone camera image goes out as the sensor captured it (landscape,
however the phone is held), in the declared camera frame its pose and depth
use. Display rotation is the reader's job.

**Clocks**

- Every timestamp identifies its clock by full registry reference
  (`peer_id`, `id`, `hash`). Never infer a clock from a peer, a field name or
  the number itself.
- Timestamps are capture time under that clock, in nanoseconds from its epoch.
  Publication time is a separate event. Preserve capture time and clock through
  processing and forwarding.
- Compare clocks by full identity. Crossing clocks needs an explicit time
  transform.

## One contract per relationship, for every producer

Phones, robots and simulators expose the same thing the same way through the
SDK Catalog, so Home has one generic consumer and no per-device adapters.

A camera that can be drawn over (Home's picture-in-picture AR) provides:

- A `VideoFrame` Product whose Output has a registered `spatial_frame` (the
  camera's own frame) and a clock.
- Product metadata with `calibration` (width, height, `K`, `D`,
  `distortion_model`) matching the image as sent, and `pose_product_id` naming
  the capture-matched pose Product on the same peer. The field is
  `pose_product_id` for every producer.
- A `PoseUpdate` Product (`auki.odometry-pose/v1`) with an observation at the
  image's exact timestamp on the same clock, whose contract `from_frame` is
  the image's frame. Its `to_frame` is either a Home scene frame directly, or a
  base frame with an odometry Product and a declared transform into Home.

Pose payloads: `translation: [x, y, z]` in destination-frame units and
`rotation_xyzw`, an active Hamilton quaternion mapping source coordinates into
destination coordinates. Endpoints must share units and handedness; convert
explicitly before publishing. `tracking` is `valid` with a pose or `lost`.

## Components and Products

- Construct Products through the runtime's capture APIs
  (`capture_buffer`, `capture_buffer_with_metadata`) or `imported_buffer`, not
  struct literals. A local Component is not reachable until its Product is
  exported on the peer's `ComponentProtocolEndpoint`.
- Catalog metadata is publisher-supplied discovery data, not authorization and
  not a guarantee the observation is still retained. Compare its
  `source_sequence` with a fetched observation when an exact match matters.
- Forwarding a Product preserves its source Output, sequence, time, clock and
  frames.

## SDK boundaries

- `auki-sdk` owns peer lifecycle and connection coordination. P2P carries
  application data; DMS owns task state. Do not add P2P task dispatch.
- A Peer ID proves transport identity, not permission. P2P admission does not
  authorize an application operation: every Operable checks its own caller.
- Never relax signature, issuer, audience, expiry, peer, domain or scope checks
  to make something work.
- Never log or commit passwords, App secrets, tokens or peer identity files.
- Preserve cancellation, bounded retries and awaited shutdown. No background
  task, discovery registration or relay booking outlives its owner.
- Source support is not deployed support. Changing a wire format, schema or
  metadata field needs every producer and consumer moved together.

## Working here

- Home, the phone core (`src/mobile*.rs`, compiled into the apps) and the
  runner share this crate. A struct Home publishes is decoded by the phone's
  build of the same struct, so a field one side adds is dropped by the other
  until both are rebuilt.
- Default to local fixtures. Running Home, a phone or a robot against a shared
  Domain publishes to other people's scenes.
- Do not run `rustfmt` over whole files; it rewrites unrelated code.
