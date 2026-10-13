# Registry-backed frame consumer

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
Home keeps one SDK continuing-observation subscription per exact held Product,
shared by scene poses, depth and camera AR. `FromSequence(0)` streams the retained
tail and continues with pushed changes, without a fetch/subscribe race or a
network request for each pose. The SDK's imported `RetainedProduct` is the cache;
Home does not maintain a separate pose queue. Idle held Products need no heartbeat
and have no age expiry. A missing older retained state means that capture cannot
be placed; future transformations never apply backward.

Home validates each original Output, source sequence, timestamp and payload.
An explicit gap discards the pre-gap state for lookup and resynchronizes from the
available tail. A transport error invalidates the cache and reconnects with a
bounded 1–8 second backoff (5 second connection deadline). Terminal Products remain
unavailable until explicitly rediscovered/rebound; Catalog removal, changed
definitions, routes or Product identities tear down the old subscription.
Shutdown cancels and awaits workers. Retention is bounded to 1,024 states / 1 MiB
per Product, with at most 128 subscriptions. Evicted history is unavailable,
not extrapolated from a newer state.

Catalog metadata remains discovery data, never the transform itself. If its
advertised source sequence is ahead of the validated stream, reads fail closed
until the stream catches up, so a known pending revocation cannot be ignored.
Delivery is asynchronous: an idle subscription cannot immediately detect a
silent network partition or a not-yet-received change. Home invalidates on SDK
gap/error/closure events; it does not invent clock synchronization or liveness.

## Depth

Consume depth-points/v1 with descriptor {frame, clock, session_id}. It is a
bounded camera-space point cloud, optionally with one RGB color per point.
Home polls the latest retained capture, so this is deliberately lossy under
load, not an exhaustive recording. It applies scene_from_native *
native_from_camera at capture time and integrates once per exact Product and
sequence. The legacy posed-depth ingestion operation remains disabled.
No past geometry is replayed or back-propagated when calibration changes.

## Camera AR

Camera discovery follows the exact frame and clock to a sampled compact pose
Product. Intrinsics are fetchable camera Product metadata, not per-image pose
payloads. AR uses the same capture-time interpolation and held relationship
rules as depth, converts the declared source axes to optical axes, and projects
notes and paths. Legacy pose decoding and camera following remain disabled.
Presentation-only rotation does not change these frame relationships.

The phone currently emits its held scene relationship from the depth-sharing
path. With depth sharing disabled there may be no relationship to consume;
Home does not invent one from the peer's presence or alias.

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
