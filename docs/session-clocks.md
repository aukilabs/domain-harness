# Session clocks and consumed history

Every long-running peer owns `auki_time::SessionClock`. The clock definition is
served through Registry and the Catalog advertises an exported SDK time-transform
Product (`spatial-harness.time-transform/v1`). Its immutable descriptor binds the
full source and destination clock definitions to its Output identity. The UTC
destination is UTC anchored from that peer's local Unix realtime clock at session
start, not a claim of global sync.

The Product uses the SDK's three-read sampler and `TimeTransform` math. One held
sample contains the session-start offset and read-window uncertainty; the
Observation envelope supplies its session timestamp. Later host wall-clock
corrections do not change that anchor or rewrite old captures. Clock
definitions and transform Products are discoverable through the same authenticated
SDK/libp2p paths as other Products. No HTTP peer-side clock endpoint is introduced.

To resolve a data stream's time, read its Output's full `clock` reference, resolve
that exact Registry definition, then find the Catalog time-transform Product
whose `from_clock` has the same owner, ID and hash. Fetch its held anchor through
the SDK. Do not pick a peer's first clock or guess from a camera/Product name.
The session identity is part of the registered clock; a new session cannot reuse
an old history or anchor. Forwarded observations keep their original clock.

## Capture, not arrival

Native capture clocks are converted at the producer boundary. The adapter reads
the native clock between two reads of the exact SDK session clock. Queued frame
timestamps never participate in measuring the offset. Conversion preserves
processing delay and requires measurements bracketing the original capture;
captures before the session or outside retained clock history are not relabelled.

The local adapter retains ten seconds plus a predecessor (at most 4,096 samples),
rejects read windows over 1 ms, and rejects an offset step over 5 ms. Its additional
capture-conversion budget is declared separately from the UTC read-window error.
This budget is not the sensor's intrinsic timestamp accuracy. A native clock reset
requires a new clock relationship/session; ordinary spatial tracking loss does not.
An already converted held calibration keeps its original converted timestamp.

UTC offset measurements alone do not synchronize different machines. Home accepts
same-clock comparisons directly, and different clocks with an explicit common UTC
definition. Cross-host UTC composition additionally requires declared UTC accuracy
bounds. Missing bounds or anchors and ambiguous identities fail. Captures in
the same session use its fixed offset, without waiting for another UTC sample. Spatial
pose queries still need retained capture-matched samples; Home never silently
substitutes the latest pose or extrapolates it.
AR capture matching includes both native-conversion budgets and both UTC anchor
errors, plus cross-host UTC accuracy where applicable. It rejects a total over
20 ms. Same-clock queries need no UTC round trip. A detected native clock reset is
latched: a later return to the old offset cannot revive that session's mapping.

## Home

Home continuously imports declared pose and time-transform Products into SDK
retained buffers. Viewer, Qwen pose lookup, camera presentation, AR and depth share
the pose history. AR queries use the SDK's atomic time brackets; they do not fetch
pose history from the peer for each image. Catalog metadata is not pose data.

Transport interruptions do not erase the fixed session UTC anchor or already
received sampled poses. Retained brackets may still resolve historical captures,
but disconnected samples never count as a live peer, and interpolation cannot
cross missing source sequences. Held spatial calibration remains unavailable
during an unknown disconnect until its transition history is re-established,
because it may have been revoked while Home was offline.

Consumed sampled poses, camera frames, point clouds, structured state and scene
reads target a ten-second **source capture-time** window. They retain original
Output, sequence, timestamp, clock and payload. UTC conversion is a separate query
over retained time-transform Products, not a rewrite of source Observations.
Camera and pose responses expose UTC conversion status and actual retained span.

Buffers also have hard entry/byte bounds. A newly opened stream, a reconnect or a
high-bitrate stream can have less than ten seconds; a shorter span is reported,
not represented as complete history. Home consumes camera pixels on demand, with
one combined budget of eight camera histories for live PiPs and requested
snapshots. Qwen snapshots reuse an open PiP subscription and wait for its next
fresh frame without blocking the preview service. Otherwise the selected camera
is opened only for the one-shot capture, then its subscription closes. Its exact
SDK observation envelopes remain in a ten-second imported binary history;
retention does not keep the camera active. Snapshot caches expire on Home's
existing maintenance tick, use the encoded byte length for their memory bound,
and never serialize image bytes as JSON. Home does not open every advertised
camera to fill buffers. Held calibration keeps bounded transition/revocation history even
when its last measurement predates the ten-second sampled-data window.

## Rollout and validation

Onboarding also uses these registered session clocks. The host's marker reply
declares its full clock reference; both phone and Diagnostics joiners resolve
that exact clock's UTC-transform Product and both Registry definitions. The
bounded clock-read exchange returns SDK session time, never AR uptime, and
measures the difference between the peers without changing their fixed UTC
anchors or claiming global UTC accuracy. Requests and replies bind the pairing
session, clock owner/ID/hash, capture time and uncertainty. Host pose matching
requires retained brackets covering the full uncertainty interval; changed clocks,
tracking gaps, expired history and extrapolation are refused. Native timestamps
remain only as private phone calibration evidence at the app boundary.

The timestamp-bearing onboarding operation payloads are v2. Host and joiner must
be rebuilt together; v1's unlabelled native-clock payloads are not accepted.
Invitations retain their existing QR format. No networking outside SDK/libp2p
or clock inference from device names is introduced.

Producer changes require rebuilding the corresponding application; adding source
support does not update running phones or robots. A new SDK session clock changes
the Product/clock identities and must be rediscovered. No persisted peer identity,
scene, note, or calibration file needs deletion.
