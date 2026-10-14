# Registry-backed camera AR

Home PiP uses one consumer (`registry_ar`) for all discovered camera publishers.
Neither aliases, device kinds, Product names nor legacy mapper metadata select a
code path. No peer-specific HTTP data path or inline-pose fallback is provided.

## Camera contract

The existing `spatial-harness.camera-contract/v1` Product metadata contains
`{sensor, frame, clock, session_id}`. Each reference includes owner, ID and exact
SDK registry hash. The Output binds this descriptor through
`camera-<SDK manifest_hash(descriptor)>`. Its registered frame, clock, image
dimensions and encoding must agree. Registry v3 supplies the immutable numeric
camera calibration; Home caches definitions by exact reference.

Publishers using only legacy inline calibration/pose metadata must migrate to
this shared contract before AR is available. A changed calibration requires a
new descriptor-bound Output, not replacement numbers under an unchanged identity.

## Transform chain

Home discovers compact-pose Products by schema and exact producer identity. It
finds a unique directed chain from the camera frame to one local Home scene,
up to eight edges and 64 candidate transforms. Every edge must match the
camera's full clock reference and session. Different clocks require a declared
time-transform implementation; there is no implicit UTC/arrival-time conversion.
Ambiguous routes or scene targets are errors, not shortest-path guesses.

Authority policy: the authenticated camera publisher may assert relationships
out of its own frames into Home. Exact registry references prove identity, not
localization accuracy. Unrelated peers cannot substitute transforms for it.

At the image capture timestamp:

- A sampled transform needs an exact sample or adjacent valid samples with at
  most a 250 ms gap; translation is interpolated and rotation uses SLERP.
- A held transform uses the latest retained state at or before capture, including
  revocation. It does not expire merely because the portal leaves view.
- Loss, gaps, Output replacement, unresolved definitions and session resets fail
  closed. Missing pose bounds may be retried within the existing two-second PiP
  synchronization budget, using the same retained image. Never extrapolate.

The composed pose is explicitly converted from the declared camera axes to the
projection library's optical convention. Presentation rotation stays in the
existing presentation layer, applied equally to image and overlays. Capture
timestamps and sequences in AR responses are decimal strings.

Camera subscriptions remain demand-driven and alive for the existing PiP lease.
Transform histories are read through authenticated SDK Product requests; Catalog
and registry resolution use Home's discovery cache, not per-pose Catalog calls.
The checkbox indicates a supported resolved camera calibration, not guaranteed
tracking. Missing alignment produces a specific error when rendering.

## Validation

`cargo test --test registry_ar` uses isolated authenticated loopback peers, real
Registry and Product protocols, and arbitrary publisher/product names. It covers
two- and three-edge chains, interpolation, future alignment exclusion, missing
definitions, wrong clocks, competing paths, and revocation. Unit tests exercise
loss, gaps, interpolation limits and absence of extrapolation.

This does not change the diagnostics runner's tracking-loss/alignment lifecycle.
Live Home deployment and visual overlay accuracy remain separate acceptance steps.
