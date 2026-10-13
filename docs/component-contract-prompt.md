# Prompt: implement declarative Auki SDK components

Update this component to separate immutable interpretation metadata from changing observations. Use the Auki SDK Component/Output/Product APIs and authenticated Registry v3; do not introduce another registry transport or repeat full definitions in measurements.

## Publisher requirements

- Declare a typed, versioned output schema. Do not change the wire shape under an existing schema name.
- Publish frame and clock definitions through SDK Registry v3. References must include owner peer ID, registry ID, and exact canonical SDK hash. Use SDK hashing and validation, not ad-hoc JSON hashing.
- Put stable interpretation in the fetchable output contract or an explicitly bound descriptor: source/destination frame references, clock reference, and session identity. Camera dimensions and intrinsics likewise belong in registry/configuration declarations, not each frame event.
- Bind any descriptor to the exact output identity. Mutable, unbound Product metadata is not sufficient: a receiver must detect replacement.
- Keep observations minimal: measurement timestamp and sequence in the standard SDK Observation envelope; only tracking status and transform in the pose payload. Do not duplicate the envelope's peer/output references or timestamp inside the payload.
- For a valid pose, use finite translation and a normalized Hamilton quaternion in XYZW order. The active transform maps source coordinates into destination coordinates; translation uses destination-frame units. For lost tracking, send no transform.
- Keep robot poses in their native odometry frame. A robot-to-Home alignment is a separate transform, never an implicit coordinate conversion.
- Use the same compact pose profile for frame relationships: `lifetime: held` for a relationship that stands until replaced or lost, and omitted/default `sampled` for moving poses. Source frame and capture clock belong to the publisher; the destination may belong to another peer, whose exact Registry entry must be fetched. See `docs/frame-registry.md` for timestamp, revocation, authority and frame-epoch rules.
- New physical odometry origin means a new destination-frame identity and output/session, even if axes are unchanged. End the old output. Never silently reuse a frame after reset.
- Retain the exact registry definitions required by retained observations. Do not advertise an output whose definitions cannot be fetched.
- Keep buffers and queues bounded. Network backpressure must not block BBOS health or pose capture.

## Receiver requirements

- Discover by exact datatype/schema and producer reference, not peer names, aliases, slot names, or guessed coordinate conventions.
- Validate Product → Output → descriptor binding, then fetch exact referenced registry documents using the SDK typed RegistryClient. Verify owner, ID, hash, and frame/clock compatibility before interpretation.
- Cache definitions by exact references, not names alone. Invalidate resolved output state when the Product/output contract changes.
- Unknown or unavailable definitions must result in an explicit unresolved state, never guessed metadata or a fallback to the newest definition.
- Preserve u64 timestamps and sequence values. JavaScript display APIs should expose these as decimal strings to avoid precision loss.
- Separate tracking loss from stale/disconnected transport. Do not portray arrival age as capture latency without a known clock relationship.
- Do not extrapolate across tracking loss or odometry resets.

## Current Home/diagnostic profile on SDK PR425

The upstream `auki.odometry-pose/v1` payload requires an inline contract. Do not send a compact payload under that schema.

This repository defines the shared application profile in `src/compact_pose.rs`:

- Datatype and schema: `spatial-harness.compact-pose/v1`.
- Product metadata schema: `spatial-harness.compact-pose-contract/v1`.
- Descriptor: `{from_frame:{peer_id,id,hash},to_frame:{peer_id,id,hash},clock:{peer_id,id,hash},session_id}`.
- Output clock: the same exact clock reference.
- Output registered spatial frame: destination frame.
- PR425 has no generic descriptor-reference field. This profile binds the descriptor with `output_id = "poses-" + SDK manifest_hash(descriptor)`; the SDK OutputManifest hash therefore binds it transitively. Consumers must validate this rule. This is an application convention, not an upstream SDK standard.
- Valid payload: `{"tracking":{"status":"valid","pose":{"translation":[x,y,z],"rotation_xyzw":[x,y,z,w]}}}`.
- Lost payload: `{"tracking":{"status":"lost"}}`.

No frame axes, units, clock definition, session ID, or registry references occur in individual pose payloads. The SDK envelope still carries its standard output reference, sequence and timestamp; this change does not redesign that transport envelope. PR425 also embeds the destination definition in the fetchable OutputManifest; that is configuration, not per-sample payload.

Home's inspector accepts only this profile. Legacy odometry v1 is unsupported. Home's unmigrated pose-dependent consumers intentionally fail closed; do not restore them through adapters, guessed definitions, or legacy fallbacks. Other AR/localization consumers must explicitly adopt the new contract before being enabled.

## Acceptance tests

Prove that samples contain no static definitions; descriptor changes change output identity; malformed transforms, incorrect registry hashes/owners, and conflicting output contracts are rejected; registry outages remain unresolved; tracking loss contains no stale pose; and the catalog contains only the requested Products. Test against a real authenticated publisher/consumer pair before claiming deployment interoperability.
