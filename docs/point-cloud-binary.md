# Compact point-cloud observations

The additive `spatial-harness.depth-points/v2` Product carries compact binary
measurements through the SDK's authenticated component observation stream over
libp2p. It is not a WebSocket, JSON array of bytes, or base64 envelope. The SDK
codec is explicitly named `spatial-harness.depth-points-packed/v2`.

Existing `spatial-harness.depth-points/v1` JSON Products remain readable. A
producer and its consumer must both support v2 before switching that Product;
v2 is not sent to an old JSON decoder under a v1 schema.

## Capture identity

The SDK Observation envelope retains the original source sequence and capture
timestamp. Its exact Output declares the capture clock and registered spatial
frame. Neither is replaced with send time or arrival time.

Each v2 packet repeats the small, bounded `PointsContract` descriptor:
`{frame, clock, session_id}`. Frame and clock are full registry references with
owner, ID and definition hash. The consumer checks this descriptor against the
Output, including its contract-derived `points-<hash>` ID. Changing schema changes
the Output manifest identity; Product names are not capability or frame hints.

This descriptor in the first data packet allows a consumer to discover and
subscribe to an initially empty Product before any points exist. Catalog
metadata is optional discovery information, not a requirement for starting
demand or an authority to reinterpret the received data.

The measured coordinates remain in their declared sensor frame. Home uses its
same capture-time transform resolver for every peer to reach its scene frame:
registered extrinsics, sampled motion, and held scene alignment as needed.
Binary packing does not change transform ownership, interpolation bounds,
tracking-loss checks, or reset semantics. Missing transforms do not authorize
using a latest pose or extrapolation.

## Packet layout

All integers and float32 coordinates use little endian. The fixed header is 16
bytes, followed by a descriptor and interleaved measured-point records.

| Offset | Bytes | Meaning |
| --- | --- | --- |
| 0 | 4 | ASCII `APC2` |
| 4 | 1 | Flags: `0` for XYZ, `1` for XYZ + RGB |
| 5 | 3 | Reserved; must be zero |
| 8 | 4 | Point count, unsigned 32-bit |
| 12 | 4 | Descriptor byte length, unsigned 32-bit |
| 16 | variable | UTF-8 JSON `PointsContract`, at most 4,096 bytes |
| after descriptor | 12 or 15 per point | X, Y, Z float32; then optional R, G, B unsigned bytes |

The numeric payload is **12 bytes/point**, or **15 bytes/point with color**.
Coordinates are metres and colors are sRGB. Float32 exactly preserves float32
inputs and avoids a further float16 quantization step after sensor-frame
conversion. Producers starting from higher-precision values explicitly round
to float32 when opting into v2; v1 is unchanged.

A capture contains 1–2,048 points; zero points, nonfinite coordinates, coordinates
outside ±100 metres, unknown flags, nonzero reserved bytes, malformed contracts,
incorrect lengths and trailing bytes are rejected. Maximum packet size is
34,832 bytes. The decoder checks bounds before allocating points. Timestamps do
not appear twice in the packet.

This profile does not expand source sampling or fill gaps with invented points.
The producer's capture limit and measured-point selection remain explicit.

## Demand and lifecycle

The Product is discoverable while idle. An active continuing-observation
subscription supplies demand; merely listing the Catalog does not start point
publication. Dropping or closing the subscription removes that demand. The
producer's `set_enabled` Operable remains an independent explicit override:
disabled means no new point publication even if a subscriber exists.

Home's active voxel mapper is itself a consumer, so it supplies demand without
the browser needing to display points. Turning off publication does not clear
Home's already accumulated voxels and does not disable BBOS depth processing.
Subscriptions and buffers remain bounded, use SDK sequence/gap/closure handling,
and preserve cancellation. No application transport bypasses SDK/libp2p.

## Verification

`tests/depth_points_binary.rs` covers both record forms, exact source float32
round trips, format and allocation bounds, malformed packets, size reduction,
and unchanged v1 JSON decoding. Authenticated end-to-end stream tests separately
exercise the publisher, SDK codec, capture timestamps, demand lifecycle, and
Home's transform chain.
