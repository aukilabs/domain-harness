# Qwen tooling audit

Scope: Home's advertised Qwen tools and their browser/SDK consumers. No robot
movement, live scene writes, or changes to transport authorization for testing.

## Corrected paths

- `inspect_scenegraph`: live locations use the viewer's exact Product/hash pose
  consumer, not phone presence. Missing, stale, lost, ambiguous, or differently
  aligned poses return reasons instead of cached coordinates.
- `point_relative_to_peer` and peer references in
  `create_note_relative_to_view`: consume the same live pose, deriving directional
  axes from Registry definitions rather than phone/robot labels or fixed axes.
- `raycast_peer_camera`: selects an advertised camera Product and obtains PiP's
  capture-matched calibration/transform chain over SDK libp2p. A selected-scene
  mismatch fails. No surface hit means no placement; the subsequent write must
  use the server-computed hit.
- `inspect_sensor_frames` and `attach_camera`: advertise Registry camera and
  sampled pose Products. Following supports both; exact Product identity,
  selected Home frame, fresh valid tracking, and declared axes are required.
- Camera AR/picking/following reject a frame that becomes stale during transform
  resolution. Camera history gaps never authorize extrapolation.
- Geometry tool discovery includes peer-relative and camera-ray queries.
- Tool selections are deduplicated, schema descriptions retain validation
  guidance, and JSON-encoded argument objects are normalized consistently.
- Repeated reads can refresh live state or recover from optimistic-concurrency
  conflicts; the overall bounded tool loop remains in place.
- Peer renaming recognizes advertised aliases without requiring the word peer.
- Camera selection guidance asks the model to choose the best advertised match,
  including obvious typos, instead of routinely asking which camera.
- Failed camera capture returns a tool error and permits bounded rediscovery and
  retry with exact advertised Product IDs; it never supplies an invented image.

## Audited paths retained

- Catalog queries: paginated, bounded SDK snapshots, with cached/unavailable
  status; reading an operable never executes it.
- Image descriptions: a selected, newly captured Product observation; bounded
  image decoding and a separate tool-free vision pass.
- Move/detach viewpoint: browser-local actions bound to scene/viewer frame.
- Notes and volumes: typed arguments, selected-scene binding, expected-value
  concurrency checks, protected system labels, and explicit bulk-delete checks.
- Voxel clearing: explicit clear request and Home-owned local scene required.
- No navigation/drive operable is exposed by this tool set.

## Validation policy

Use library regression tests, authenticated loopback camera/transform fixtures,
browser-control unit tests, and local-model tests with synthetic handlers. Never
exercise edit tools against a user's live scene merely to validate this audit.
Model-driven tests are opt-in and their pass/failure must be reported separately
from deterministic tests. A passing tool implementation is not a guarantee that
every natural-language phrasing will be interpreted correctly.

## Validation results (2026-10-14)

- 96 library tests, 2 registry AR integration tests, and 9 browser-control tests passed.
- The 15-scenario local Qwen run passed 14 scenarios initially. The failed image
  description exposed a capture-retry issue; its targeted rerun passed after the fix.
- A separate synthetic live-peer distance test passed without scene edits.
- Integrated release build passed. The full repository integration suite was not run.
