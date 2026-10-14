# Android system DNS

Source: auki-sdk commit 3342d589605846f5f45c1de5430dd22923555de2,
core/auki-p2p. Workspace metadata/dependencies are copied from that revision.

Android now selects the existing iOS SystemDnsTransport path, which uses Tokio
lookup_host / the OS resolver rather than reading /etc/resolv.conf. Desktop and
explicit DNS-config paths are unchanged. No public DNS server is hardcoded.
Remove this local Cargo patch once the pinned SDK includes Android support.

## Cancel-safe authenticated protocol admission

Managed protocol servers receive raw inbound streams separately from their
authentication. Authentication runs in a bounded owned worker, so completion
of another request cannot cancel a partially completed handshake. Previously
the server selected `incoming.accept()` against handler completion, dropping
the in-flight stream when another handler finished (client EOF/closed stream).

The independent `diagnostics/connection-peer` Cargo workspace must apply this
patch too: patches in Home's workspace are not inherited. Its lockfile must
resolve `auki-p2p` to this path. No token checks or wire contracts change.
Regression: `completing_operations_do_not_cancel_other_stream_authentication`.

Exact-route opens also serialize handshake admission per target peer and
protocol across Node clones. Established streams remain concurrent; each new
stream still authenticates. Weak gate entries are pruned and permits release
on cancellation. This prevents concurrent local readers overrunning the small
libp2p-stream inbound queue. The regression has no caller-side admission lock.
