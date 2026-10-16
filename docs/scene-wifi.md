# Scene Wi-Fi for onboarding

On the Home computer at `http://127.0.0.1:4329`, select a local scene,
open **Settings → Scene Wi-Fi**, enter its SSID
and password, and select the peers allowed to host onboarding (for example Dan's
Annotator). Save. Connect the host to Home before opening the editor so it is
listed; saved grants remain visible even when their peers are offline.

The password field is blank on read. Leave it blank to retain the password for
the same SSID; enter a replacement to change it. **Open network** explicitly
clears it. Supported networks use the existing invitation contract: open or
WPA personal (8–63 password bytes), plus an optional hidden-network flag. This
does not add enterprise Wi-Fi, captive-portal login, or robot OS provisioning.

## Storage and access

Each stable scene ID owns `.settings/wifi/<scene-id>.json` under Home's scene
directory, independently of its map frame. Origin/scenegraph resets preserve
both Wi-Fi and host permissions. Removing settings clears both. Deleting the
scene retires the file with its other scene-owned files. Files are owner-readable
and writable only on Unix, in an owner-only directory; they are not encrypted.

Neither spatial snapshots, scene Products, Catalog metadata nor Qwen inventory
contain credentials. The browser management endpoint
`/api/local-scenes/<scene-id>/wifi` requires an actual loopback client socket plus
Home's existing Host/same-origin management checks. It ignores forwarded headers;
a LAN caller cannot grant itself credential access by spoofing Host. Other Home
UI features remain LAN-enabled. This endpoint returns only SSID, hidden flag, password-present flag and allowed peer
IDs, with `Cache-Control: no-store`. Password edits are explicit keep/set/clear;
the UI never saves credentials in browser storage.

Every scene exports a standard SDK Component `scene-wifi:<scene-id>` with the
Operable `get_for_onboarding`:

- Request: `{}` (`spatial-harness.scene-wifi-request/v1`).
- Reply: `{ "wifi": null }` if unconfigured, otherwise the existing invitation
  Wi-Fi object `{ "ssid": ..., "password": ...?, "hidden": ...? }`
  (`spatial-harness.scene-wifi-reply/v1`).
- Home uses the transport-authenticated caller Peer ID, **not** a request field
  or caller Component name. Only IDs explicitly allowed for that scene receive
  configured credentials. An authenticated but unapproved peer is denied.
- Revoking permission affects subsequent reads immediately. It cannot recall a
  password already received or a QR already photographed.

This is the same operation for every peer, with no device-type checks. Peer
requests use the existing SDK Catalog and authenticated libp2p path, never the
browser HTTP endpoint. Existing SDK Domain and transport checks remain intact.

## Annotator

After alias entry and calibration, the iPhone bridge retrieves the scene Wi-Fi
through that operation before constructing the first invitation QR. The UI
shows **Retrieving scene Wi-Fi from Home…**, with bounded failure and Retry.
The request runs off the UI thread. Cancellation, changed connection or lost
calibration prevents late results from producing a usable invitation.

An unconfigured scene yields the usual invitation without Wi-Fi. Missing
capability, denied permission or a transport error does **not** silently omit
configured credentials or fall back to stale local settings. Upgrade Home and
Annotator together. The QR's existing v1 format and dynamic-marker contract are
unchanged; the recipient still needs its own valid SDK/domain credentials.

The QR contains the password deliberately so an offline joiner can read it.
Treat it as a credential, not a public connection profile; never log its payload.
