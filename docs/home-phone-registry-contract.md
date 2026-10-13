# Home's declared-frame consumer

Wire profile: [frame-registry.md](frame-registry.md), shared with PR #40.
Home uses Registry v3 definitions and compact pose Products. The separate
phone-home-alignment profile is no longer accepted.

## Authority and discovery

Resolve by exact frame owner/ID/hash, session and clock, never Product or
component names. Home accepts a same-Domain authenticated publisher's claim
about its own native frame. This does not independently prove calibration
accuracy. The destination must equal Home's scene registry entry, including
owner, ID and hash. Ambiguous paths are rejected; no first-Product authority.

Home fetches definitions during Catalog discovery. It requires them before
using pose or depth. Missing definitions are retried, without inline fallback.

## Time

Native motion is sampled. The viewer only renders recent arrivals (under two
seconds); depth uses the capture clock, exact samples or consecutive valid
brackets at most 500 ms apart, translation interpolation and shortest-arc
quaternion SLERP. No extrapolation, crossing lost, mixing sessions, or comparing
arrival time to capture time.

Session-to-scene is held. Select the latest retained state at or before capture
time, including lost. No age expiry and no walking backward past revocation.
Home rereads retained history and fails closed on fetch failures. A missing
older retained state means that capture cannot be placed.

## Depth

Consume depth-points/v1 with descriptor {frame, clock, session_id}. It is a
bounded camera-space point cloud, optionally with one RGB color per point.
Home polls the latest retained capture, so this is deliberately lossy under
load, not an exhaustive recording. It applies scene_from_native *
native_from_camera at capture time and integrates once per exact Product and
sequence. The legacy posed-depth ingestion operation remains disabled.
No past geometry is replayed or back-propagated when calibration changes.

## Verification and remaining lifecycle work

Devices can inspect both sampled and held compact Products; the inspector
labels lifetime explicitly. Native Maps trails exclude held relationships.
The main scene displays only sampled poses with a resolvable held Home edge.

Origin reset is restart-bound. The snapshot-checked reset operation queues a
durable intent and returns `restart_required: true`; the running scene stays
unchanged. On startup, before any Products are mounted, a recoverable journal
clears portals, assigns `local-scene:<scene>:epoch-<UUID>`, and rebinds saved
notes, volumes and paths. Their numeric coordinates and IDs are retained, not
physically realigned. In-memory geometry is discarded on restart.

All scene Outputs are freshly mounted in the new frame. Old held transforms
cannot match it, even with identical axes and units. Peers must fetch the new
Home connection/reference and localize again. Completed journals retain original
authored data. Interrupted migrations resume; conflicting edits fail startup
rather than silently overwriting them. Ordinary restarts do not reset origins.
