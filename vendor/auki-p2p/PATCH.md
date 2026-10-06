# Android system DNS

Source: auki-sdk commit 3342d589605846f5f45c1de5430dd22923555de2,
core/auki-p2p. Workspace metadata/dependencies are copied from that revision.

Android now selects the existing iOS SystemDnsTransport path, which uses Tokio
lookup_host / the OS resolver rather than reading /etc/resolv.conf. Desktop and
explicit DNS-config paths are unchanged. No public DNS server is hardcoded.
Remove this local Cargo patch once the pinned SDK includes Android support.
