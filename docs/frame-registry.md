# A peer's frame registry

A peer is a peer. Any peer can fetch any other peer's frame registry: the
frames it defines, and the transforms it knows between frames. Nothing here is
specific to a phone, a robot or Home.

With it, one peer can ask another "do you know how to get from your space to
mine?", and consume what that peer measured without being told how in every
message.

## Two parts

**1. Frame and clock definitions: SDK Registry v3.**
Every peer mounts the SDK's registry endpoint. `List{kind: Frame}` returns the
frames it defines; `Get{kind, id, hash}` returns one exactly. A reference is
always `{peer_id, id, hash}`: a peer serves only the definitions it owns, and a
frame another peer owns is fetched from that peer.

**2. The transforms it knows: Products under one profile.**
Every transform a peer knows is a Product under the compact pose profile in
`src/compact_pose.rs`:

- Datatype and schema `spatial-harness.compact-pose/v1`.
- Product metadata `spatial-harness.compact-pose-contract/v1`:
  `{from_frame, to_frame, clock, session_id, lifetime}`, each frame and the
  clock a full reference. `lifetime` is `sampled` (the default, omitted on the
  wire) or `held`.
- The contract is bound to the Output: `output_id = "poses-" + manifest_hash(contract)`.
  The Output's registered frame is the destination frame.
- Observation at a capture time on that clock:
  `{"tracking": {"status": "valid", "pose": {"translation": [x, y, z], "rotation_xyzw": [x, y, z, w]}}}`
  or `{"tracking": {"status": "lost"}}`. The active transform maps `from_frame`
  coordinates into `to_frame` coordinates.

A pose is a transform that changes often. How one peer's frame sits in another
peer's is a transform that changes rarely. They are the same contract, and
`lifetime` says which, so no reader infers it from a Product's name:

- `sampled`: a measurement of something moving, true at its timestamp. Read by
  interpolating between neighbours; an old one is stale.
- `held`: a relationship that stands until replaced or declared lost. It does
  not go stale with age: a session-to-scene transform survives the Portal
  leaving view.

- A peer measures from a frame it owns, on a clock it owns.
- The destination may be a frame another peer owns. That is how a peer says
  where its space sits in someone else's.
- A new origin, or a different destination, is a new Output, never a relabelled one.
- A peer that has not measured a transform publishes none. One it stops
  vouching for says `lost` once.

## Does peer A know A-space to B-space?

1. Fetch A's Catalog.
2. Take the Products whose metadata schema is the compact pose contract.
3. Look for one whose `from_frame` is A's frame and whose `to_frame` is B's
   frame, by full reference. A chain of them through intermediate frames works
   the same way.
4. Fetch any definition not already held from the peer that owns it, and check
   its hash.

## Rules for a reader

**Clocks.** Timestamps are compared only when they are on the exact same clock
reference (peer, ID, hash). Across clocks a reader needs a declared time
transform. Arrival time is never a substitute.

**Lost invalidates.** Use the latest state of a transform at or before `t`. If
that state is `lost`, refuse. Do not search backwards for an older valid one.

**Interpolation.** For a `sampled` transform, interpolate translation linearly
and rotation along the quaternion arc, only between two `valid` samples of the
same Output: same session, frames and clock. Never across a `lost`, a reset, or
more than the reader's stated maximum gap. Do not extrapolate past the last
sample.

**Ambiguity.** Two peers, or two Products, may offer competing transforms
between the same frames. A reader rejects the ambiguity or applies an explicit,
stated authority policy. It never takes whichever it saw first.

**Identity is not accuracy.** A `to_frame` that matches the owner's peer ID,
frame ID and registry hash proves which frame is meant. It does not prove the
transform is right. Whether to accept a peer's transform into a shared map is
the reader's trust policy, stated separately: which peers, on what evidence,
within what bounds.

## Rules for a publisher

**A new origin is a new frame.** A frame's registry hash covers its axes and
units, not where its origin physically is. If a peer moves a frame's physical
origin, it must publish a new frame ID. Otherwise every transform into the old
frame still matches by reference and is silently wrong. This applies to a scene
whose origin Portal is re-established as much as to an AR session that resets.

**Vouch or withdraw.** A peer that has not measured a transform publishes none.
One it stops vouching for says `lost` once, on the same clock.

## What a phone publishes

| Product | From | To | Lifetime |
| --- | --- | --- | --- |
| `phone-pose:<session>` | `phone-camera:<session>` | `phone-ar:<session>` | `sampled` |
| `frame-transform:phone-ar:<session>` | `phone-ar:<session>` | the scene frame of the Home it is paired with | `held` |

Both are on the phone's `arkit-capture` clock for the AR session. The second
appears when the phone aligns on a Portal, timestamped at that Portal
observation, and is republished as the alignment is refined.

## Data in a declared frame: depth points

Depth is an example of data a reader places using the registry.

- Product `depth-points:<session>`, datatype `spatial-harness.depth-points/v1`.
- Metadata `spatial-harness.depth-points-contract/v1`: `{frame, clock, session_id}`.
  `frame` is the camera frame. No Product is named: a reader follows the frame.
- Observation at the depth frame's capture time:
  `{"points": [[x, y, z], ...], "colors": [[r, g, b], ...]}`. Metres in `frame`,
  up to 2048 points, colors optional sRGB. No pose and no transform inside.

To place a capture at time `t`:

1. Find the transform out of `frame`. It is `sampled`: take it at `t` by the
   interpolation rule above.
2. Find the transform out of that one's destination into the frame wanted. It
   is `held`: take its latest state at or before `t`, and refuse if that is `lost`.
3. Compose them and apply to each point.

All of a phone's Products for a session share one clock, so no time transform
is needed.

## What the phone still does the old way

- It still pushes each depth capture to Home's `ingest` operation with a pose
  and alignment inside. A Home that reads the Products refuses that push, and
  the phone reports "Depth published for Home to read". Remove the push once
  Home reads the Products.
- The session-to-scene transform is published from the depth path, so only
  while depth sharing is on. It should have its own path.
- Portal add and move reports still carry an alignment inside.
