# Runner onboarding through Annotator

Implementation handoff — 15 October 2026. This document specifies the intended demo; it does not mean these features are already implemented.

## Goal

Use Annotator to bring a runner onto the network, into the authenticated Auki transport domain, and into the selected Home scenegraph's coordinate frame. The user enters an alias, calibrates the phone into the selected scene, then shows the phone to the robot.

A peer is a peer: use registry-backed Components, Products, frames, clocks, and Operables. All peer-to-peer application communication must use the SDK's authenticated libp2p transport. No robot-specific Home ingestion path or peer HTTP fallback.

## Agreed user experience

1. Annotator has a button labelled **“add”**.
2. Tapping it asks for an **alias for the runner**.
3. After entering the alias, Annotator prompts the user to **calibrate the phone into the selected Home scenegraph**, using its existing scene-calibration workflow. Do not show the onboarding QR yet.
4. Once calibration succeeds, Annotator **automatically generates and displays the onboarding QR**, without another button press. It contains the connection information, including Wi-Fi credentials when needed, and is accompanied by the exact text: **“Display marker to device to begin onboarding”**.
5. The runner recognizes the QR during normal scanning, joins the specified Wi-Fi if necessary, and connects to the transport domain.
6. Once connected, the runner subscribes to Annotator's pose and alignment Products and resolves the relevant registry definitions.
7. The runner sends an authenticated **Operable request to Annotator** to display a dynamic portal for this pairing session.
8. Annotator replaces the onboarding QR with the dynamic portal. The **marker square itself is exactly 3 cm wide**, excluding its quiet zone.
9. The runner observes the portal and computes its transform into Home space using the phone's capture-matched pose and alignment.
10. The runner publishes that transform through the standard registry-backed transform contract. Home resolves the runner's live pose through it.
11. Annotator plays a **success sound once**, after usable alignment is confirmed—not merely after joining the network.

The initial onboarding QR may be large. Only the subsequent dynamic portal has the 3 cm requirement. The flow does not command robot movement.

## Demo scope and decisions

- Include Wi-Fi credentials directly in the onboarding QR. Additional credential-exposure protections are future work, not a prerequisite for this demo.
- No separate user-operated onboarding mode on the runner. Recognize our versioned provisioning payload during normal scanning.
- Deduplicate scans by pairing-session ID. Repeated frames must not repeatedly change Wi-Fi, reconnect, rename the runner, or replay the sound.
- Preserve normal SDK authentication, domain authorization, and Operable caller validation. A QR is bootstrap information, not an authentication bypass.
- Bind the alias to the runner's stable authenticated **Peer ID**. An alias is not an identity.
- Keep the selected transport domain and selected scenegraph distinct. One Home may host multiple scenegraphs.
- Do not introduce scenegraph revisions or a new peer networking stack.
- Do not make portal scanning mandatory for ordinary runner operation. It establishes the transform needed for tasks in this Home frame.
- Defer encrypted invitations, a separate enrollment service, and production provisioning-policy UX unless existing SDK authentication actually requires them.

## Responsibilities

| Owner | Responsibility |
| --- | --- |
| Annotator | Add/alias UI; phone calibration step before automatically showing the onboarding QR and instruction text; authenticated dynamic-portal display Operable; metrically accurate rendering; physical portal geometry; pose/alignment publication; progress and success sound. |
| Runner | QR decoding; session deduplication; local Wi-Fi configuration; SDK connection; registry discovery; pose subscriptions; dynamic-portal detection; alignment calculation and publication; progress/failure reporting. |
| Home | Associate the alias with the authenticated runner; consume its ordinary registry-backed transform chain; confirm that a current runner pose resolves into the selected scenegraph. |
| BBOS/local OS | Camera and tracking inputs, plus local network configuration. This is not an alternative peer-data channel. |

Reuse existing Components and Products where their semantics fit. Expose new capabilities through the standard SDK Catalog/Registry and Component endpoint. Do not create private side channels between these three apps.

Home's alias assignment and alignment confirmation must be reachable through authenticated SDK operations or Products. Verify what already exists; if an interface is missing, add it generically rather than calling Home's browser HTTP API from the runner or phone.

## Bootstrap QR

Use a compact, versioned onboarding envelope. The following is a **logical field list**, not an already-supported wire schema or an instruction to invent new SDK authentication APIs:

| Field | Purpose |
| --- | --- |
| Schema/version | Distinguish our provisioning QR from ordinary portal IDs and unrelated QRs. |
| Pairing-session ID | Correlate the QR, authenticated exchange, dynamic portal, and result; suppress duplicate scans. |
| Requested alias | Name to associate with the runner's Peer ID after it connects. |
| Wi-Fi configuration | SSID, security type, password when applicable, and hidden-network flag if needed. Omit when no network change is needed. |
| Home connection | Existing Home connection profile: peer ID, transport domain ID, usable routes, and selected scene Product/frame information. |
| Annotator connection | Phone Peer ID and bootstrap routes sufficient to reach it after joining. |
| Authentication bootstrap, if required | Only a mechanism actually supported by the pinned SDK and domain configuration. See below. |

Reuse the existing `spatial-harness.home-connection/v1` profile inside the envelope where appropriate. Its public connection details are **not credentials**. Resolve and validate the exact Home scene FrameRef through the registry after connecting; do not use a bare frame string as the spatial authority.

Keep routes to a useful bounded set and test the complete payload's QR density at the robot camera's intended distance. The initial QR must be readable without Internet access. Show it until the runner can request the dynamic portal through libp2p.

### Authentication prerequisite

The Wi-Fi password and Home profile alone do not authorize an unprovisioned runner in the Auki domain.

For the first demo, use the runner's existing SDK identity and valid domain credentials. Verify they work for the intended domain before building the UI. This gives zero manual connection entry during the demonstration, but is not proof of factory-fresh credential enrollment.

If the requirement is onboarding a completely unprovisioned runner, first confirm the SDK's supported enrollment mechanism and include its appropriate bootstrap material. That mechanism has **not been established by this investigation**. Do not copy Home's private identity/app secret into the QR or invent an unauthenticated enrollment endpoint.

### Parsing and repeat handling

- Parse a bounded, typed payload. Never execute QR contents as shell commands.
- Preserve SDK peer/domain/scope checks and validate the caller of the display Operable against the pairing exchange.
- Record the session and authenticated runner identity so late messages cannot complete another attempt.
- Ignore repeated scans for an active or completed session. A failed attempt can be retried explicitly without getting stuck in a reconnect loop.
- Do not put passwords, credentials, or the complete provisioning payload in logs.

## Joining Wi-Fi on BBOS

Read-only checks on **0125** established:

- NetworkManager is running; `nmcli` version 1.36.6 is installed.
- Two Wi-Fi interfaces were present: `wlP1p1s0` and `wlan_usb`.
- The normal `bracketbot` account requires authorization for network control and cannot modify system connection settings freely.
- Noninteractive elevated `nmcli` permission inspection worked.
- The health UI advertised a Wi-Fi page on port 9009, but that endpoint was not accepting connections. Do not depend on it as an available provisioning API.

This confirms the OS capability and a privilege requirement. **No Wi-Fi join was performed during these checks**, and other robots must be checked rather than assumed identical.

Implement joining through a narrowly privileged local NetworkManager helper or authorized D-Bus access. Do not run the entire Diagnostics runner as root. Prefer structured calls; if invoking `nmcli`, pass validated arguments without a shell and avoid exposing secrets in diagnostics.

Select the adapter deliberately, preserve known working connections, and report association/DHCP failures separately from SDK connection failures. Skip network disruption when already connected to the requested network. Handle failure without deleting the previous working profile, and verify coexistence with BBOS's Wi-Fi recovery services.

Reference: [NetworkManager nmcli documentation](https://networkmanager.dev/docs/api/latest/nmcli.html).

## Dynamic portal and phone geometry

### This is a moving reference, not a static Home portal

The dynamic portal belongs to the pairing session and is attached to the phone's physical screen. Its identity must be unambiguously associated with that session through the authenticated exchange.

- Do not save it as a permanent portal in Home's scenegraph.
- Do not make it Home's origin.
- Do not pass it through the static-portal mapper's floor/wall assumptions or gravity snapping.
- It may be tilted arbitrarily as the phone moves.
- Make sure both the runner and any other consumer can distinguish it from ordinary static portal observations.

Reuse the supported portal renderer/detector format. Confirm how it can carry or associate a session-specific marker identity; do not assume the existing portal-only decoder also parses arbitrary provisioning QRs.

### Metric rendering is necessary, but not sufficient

The **3 cm square excludes the quiet zone**. UI points, Android density units, and image pixels are not automatically physical millimetres. Use verified device-specific screen dimensions or a measured calibration and validate with a ruler on the demonstration phone. Do not silently substitute an approximate size on an unknown device.

We also need the rigid transform from the phone's tracked camera frame to the **physical displayed marker frame**, including screen position, camera-to-screen offset, screen orientation, and the detector's marker-axis convention. Rendering the right size without this extrinsic gives the wrong Home alignment.

The existing screen-share image frame is not a calibrated physical screen pose. Do not reuse it as one merely because it is called `phone-screen`.

For the first implementation, keep the portal's size, screen location, and display orientation fixed during an attempt. Publish its exact geometry before requesting detection. A layout/orientation change must terminate or explicitly update the attempt; it must not silently change a supposedly fixed transform.

Annotator must keep AR tracking and pose publication running while showing the QR/portal. Temporarily suppress screen idle and flat-phone camera sleep during the active pairing flow, then restore the normal behavior when it ends. Pose/alignment publication must not depend on depth publishing being enabled.

## Capture-time alignment

Annotator must have a valid transform into the **selected Home scenegraph before generating or displaying the onboarding QR**. Completing the alias step opens the existing calibration workflow; successful calibration automatically advances to the QR screen with “Display marker to device to begin onboarding”. Network connectivity alone must not satisfy this gate.

The QR must refer to the same scene/reference the phone calibrated into. If that calibration is invalidated or the selected Home reference changes during onboarding, pause the attempt and return to calibration before issuing an updated QR. Temporary tracking loss after calibration pauses pose matching until tracking recovers; it does not by itself discard valid calibration.

All participating frames and clocks must use full registry references, including owner and definition identity/hash. Use declared units, axes, and quaternion conventions. Do not infer geometry from a peer name or device type.

### Transform calculation

Define `T_A_B` as the transform mapping coordinates in frame **B into A**:

- `H`: selected Home scene frame.
- `P`: phone's current AR tracking frame/session.
- `C`: phone's tracked camera frame.
- `M`: displayed dynamic marker frame.
- `R`: runner's current native tracking frame/session.
- `K`: runner camera used to detect the marker.

At the same physical capture instant `t`:

```text
T_H_M(t) = T_H_P(t) × T_P_C(t) × T_C_M
T_R_M(t) = T_R_K(t) × T_K_M(t)
T_H_R    = T_H_M(t) × inverse(T_R_M(t))
```

`T_C_M` comes from the calibrated physical display geometry. `T_K_M(t)` comes from the marker detection with the correct size and capture-matched camera calibration. The other transforms come from registry-backed pose/alignment Products. Resolve the runner camera chain generically; it may include base/extrinsic frames rather than one direct edge.

Publish the result as the standard held transform from `R` into `H`, not as a special Bracketbot message. The existing compact pose contract (`spatial-harness.compact-pose/v1`, with registry-backed contract metadata) is the intended integration point; verify its exact API against the implementation branch's pinned SDK.

### Synchronization is a prerequisite

The phone and robot have different capture clocks. We need a verified mapping between them with bounded uncertainty, sufficient pose history, and capture-time sampling/interpolation. Equal-looking timestamps or both being labelled UTC are not proof of synchronization.

The implementation uses each peer's SDK session-monotonic clock and the same
Catalog UTC-transform Product used by Home. The marker reply declares the host's
full clock reference. Both joiners resolve its Registry definitions and actual
held UTC observation before measuring a bounded inter-peer offset over libp2p.
That measurement does not rewrite the session-start UTC anchors or assume the
hosts' wall clocks agree. Native camera timestamps are converted only at the
producer boundary; none are exposed as unlabelled onboarding timestamps.

Clock reads and marker-at-time operations use v2 payloads carrying full clock
references. The pairing session is not a substitute for a clock identity.
Replies must match the requested pairing session, clock, timestamp and timing
uncertainty, and pose history must cover the whole interval without gaps or
extrapolation. Old v1 hosts/joiners must be rebuilt together. The invitation QR
format is unchanged. See `docs/session-clocks.md` for the shared contract;
on-device native-clock and suspend/resume validation is still required.

- Retain the robot image/detection's capture timestamp and clock reference.
- Resolve the phone and runner poses at that physical instant through the declared clock mapping.
- Do not substitute message-arrival time, the newest phone pose, or extrapolated poses.
- Wait for bracketing pose samples within a bounded window. On failure, report synchronization/tracking status and retry observation rather than publishing guessed alignment.
- Record synchronization uncertainty and observation consistency. Reject unusable detections; do not impose an arbitrary multi-second delay when a valid solution is available.

### Alignment lifetime

Once valid, the transform remains held when the portal disappears or detection slows/stops. Temporary tracking loss pauses pose-dependent rendering/navigation; it does **not** revoke an otherwise valid alignment.

Invalidate alignment on an actual tracking-coordinate reset, a changed Home reference, or explicit withdrawal. A new physical tracking origin requires a new frame/session identity, not relabelling old coordinates. Never interpolate across tracking resets or invalid intervals.

## Operable exchange, progress, and completion

Use existing SDK operation and observation mechanisms. The following are required semantics, not prescribed method names:

1. **Request dynamic portal:** runner identifies the pairing session and selected Home reference. Annotator validates the authenticated caller, shows the portal, and exposes its identity and exact geometry.
2. **Report pairing progress:** expose stage and meaningful failure reason over a normal status Product/operation result. Correlate all results to the same session and Peer ID.
3. **Publish alignment:** runner publishes the standard held transform and continues its ordinary sampled pose Product.
4. **Confirm usable alignment:** Home's generic resolver successfully resolves a fresh runner pose into the selected Home frame. Return that confirmation through an authenticated SDK mechanism.
5. **Finish:** Annotator plays one success sound and exits pairing display. Retain the alias and alignment; stop pairing-only subscriptions/work as appropriate.

If Home has no generic confirmation interface yet, add a minimal one around its existing resolver. An Operable acknowledgement or a connected status alone must not trigger success.

Suggested visible stages:

```text
Enter alias → Calibrate phone into scene → Automatically show onboarding QR
            → Joining Wi-Fi → Connecting to domain → Showing dynamic portal
            → Matching observation and poses → Confirming in Home → Success
```

On the onboarding QR screen, display: **“Display marker to device to begin onboarding”**.

Show a concise stage and actionable error for Wi-Fi failure, authorization failure, missing registry definitions, phone/runner tracking loss, unreadable portal, synchronization timeout, or Home confirmation timeout. Provide cancel/retry, bounded waits, and cancellation of outstanding work. Preserve normal runner operation on a failed pairing attempt.

## Implementation order

1. **Verify dependencies first:** SDK/domain authorization, Wi-Fi privileges on the target robot, provisioning QR decoder support, physical screen calibration, cross-peer clock mapping, and Home's generic alias/confirmation interfaces.
2. **Prove geometry and timing:** display a measured 3 cm marker, resolve capture-matched phone/robot transforms, and verify the alignment calculation before integrating the whole onboarding UI.
3. **Build bootstrap:** Add/alias UI, scene-calibration gate, automatic QR display with the agreed instruction text, complete offline-readable QR, deduplication, NetworkManager helper, SDK connection, and authenticated session exchange.
4. **Connect Products and Operables:** register capabilities, request dynamic portal, publish held alignment, confirm through Home, and play success sound.
5. **Run the end-to-end tests below**, including failure cases. Keep diagnostics sufficient to distinguish networking, detection, tracking, and synchronization.

## Acceptance tests

- **Calibration before QR:** submitting the alias opens scene calibration and no onboarding QR is displayed while calibration is incomplete. Successful calibration automatically shows the QR for that same scene, with the exact text “Display marker to device to begin onboarding”, without another tap.
- **Network bootstrap:** a previously credentialed runner not on the target Wi-Fi reads the QR, joins, and reaches Home and Annotator without a USB data tunnel or manual connection-profile entry.
- **Already connected:** scanning on the correct network does not unnecessarily reconnect or disrupt existing streams.
- **Bad Wi-Fi credentials:** visible failure and retry; no deletion of the known working profile or endless scan-triggered reconnect loop.
- **Authentication:** knowing the QR's public connection details does not bypass SDK/domain authorization; an unrelated caller cannot take over the display session.
- **Identity and discovery:** Home discovers the runner and registered capabilities normally and associates the requested alias with the correct stable Peer ID.
- **Physical geometry:** ruler confirms the square is 30 mm; camera-to-marker extrinsics are verified, including portrait/landscape handling. Unsupported geometry reports a problem instead of guessing.
- **Stationary and moving phone:** derived alignment remains consistent when the phone is translated/rotated. Delayed samples cannot produce success using a newer, mismatched phone pose.
- **Generic consumption:** Home resolves runner pose and supported camera/point-cloud chains through the same registry resolver as any other peer, without onboarding-specific device branches.
- **Tracking interruption:** temporary loss pauses dependent consumers without discarding calibration; recovery resumes. A true tracking reset invalidates the old frame/alignment and requires a fresh solution.
- **Reference correctness:** wrong scene, changed Home origin, missing definitions, or excessive timing uncertainty prevents false success.
- **Portal lifecycle:** the displayed moving portal never becomes a static Home portal, gets gravity-snapped, or becomes the scene origin.
- **Completion:** success sound occurs exactly once and only after Home confirms a usable current pose. Repeated QR scans, duplicate requests, and late results are harmless.
- **Transport:** all peer application traffic—including aliasing, pose subscriptions, display requests, and confirmation—uses authenticated libp2p. NetworkManager calls remain local OS operations.

Coordinate deployment with the app owners. Keep **one Home server**, release build, LAN enabled; obtain approval before any restart that resets its in-memory map. Do not erase phone data or colleagues' runner installations as part of onboarding.
