# Android Annotator — local AR mapping

ARM64 Android app targeting the Pixel 7 Pro. Imports a Home connection profile,
signs in with an Auki User account, and connects over authenticated SDK P2P.
**Start local scan** opens an ARCore camera view with green voxel-center overlays.
No Portal is required. Portal alignment and annotation authoring are not implemented.

## Local scan

Connect to Home, then tap **Start local scan** and allow camera access. Move slowly
around textured surfaces. ARCore raw depth with confidence >=128 is sampled at up
to 2 Hz (0.2–5 m), using GPU-image intrinsics scaled to depth resolution and the
current frame's camera pose (ARCore reprojects raw depth to that view). Freshness
compares raw-depth measurement timestamps against the last integrated depth image,
not against the rendering frame timestamp. Repeated measurements are skipped.
No images are transmitted. See Google's
[Raw Depth guide](https://developers.google.com/ar/develop/java/depth/raw-depth).

The phone quantizes depth hits into 10 cm cells, capped at 10,000 cells, and sends
full snapshots at 1 Hz. This is surface occupancy, not a free-space or navigation
map: this milestone does not ray-clear moved objects. Stop/restart the scan to
start fresh. Tracking loss, backgrounding, or session reset clears the local map.

The Rust `android-local-mapper` component exports a retained `phone-local-voxels`
Product. Its snapshots are also pushed with their Product reference through an
authenticated Home component operation. Home checks caller ownership and bounds,
and shows **Android · local voxels (unaligned)** under Network scenegraphs. Each
session has its own OpenGL Y-up frame; nothing enters the canonical Home mapper.
The view expires after 10 seconds without fresh messages. Geometry is memory-only.

The camera can scan without connecting; in that case geometry stays on the phone.
The in-app sharing status reports Home acknowledgements, separately from local
voxel counts. Camera permission and a compatible Google Play Services for AR
installation are required. Actual capture quality still needs physical walkthrough
verification; builds/tests alone do not establish that.

The Kotlin/Compose UI calls a small JNI adapter around the existing
`spatial-portal-bridge` C ABI. Despite that crate's location under `ios/`, the
bridge and `src/mobile.rs` are platform-independent Rust. Both phone platforms
therefore use the same authentication, protocol validation, cancellation, and
scene consumption. JNI uses UTF-8 byte arrays to avoid modified-UTF-8 corruption.

The mobile peer advertises its mounted protocols and relay routes through SDK
discovery. Normal Home polls live peers' actual component catalogs separately
from mapping. Devices shows catalog loading/unavailable states, and removes
disconnected peers rather than retaining their last catalog as a live inventory.

## Port milestones

Extend this app rather than introducing a separate Android peer protocol. Keep
authentication, scene subscriptions, annotation validation, and cleanup in the
shared Rust core; Android owns camera capture, tracking, rendering, and input.

1. **Verify the connection on a phone.** Import a real Home profile, sign in,
   receive scene updates, and confirm that Home lists the phone as a peer.
   Disconnect/background must remove that live peer. Scene counts alone are not
   a substitute for this end-to-end check.
2. **Align the camera with Home.** Add ARCore tracking and known-Portal detection.
   Use the detected image's intrinsics and capture-time camera pose, and the
   Portal's metric size and pose received from Home. Do not assume that the
   phone's initial tracking frame is Home's frame or fetch Portal data from a
   backend. Clear alignment after tracking loss or a session reset.
3. **Inspect the scene in AR.** Render received Portals, voxels, and notes in the
   calibrated frame. Check scale and placement against a physical known Portal
   before enabling writes.
4. **Place annotations.** Convert a screen tap into a ray in Home's frame and
   use the existing `spatial_mobile_annotate` bridge operation. Home resolves the
   floor/voxel intersection. Preserve the note UUID for a user-initiated retry;
   report success only after Home acknowledges it.
5. **Contribute depth separately.** Check the device's supported depth capability
   before offering sharing. Keep capture-time transforms and observation
   provenance; annotation viewing and placement must work without depth support.

The local unaligned scanning path above is implemented independently of these
calibrated annotation milestones. A successful build does not prove physical AR
accuracy or successful end-to-end scene delivery on a particular network.

## Build (macOS)

The root Cargo patch vendors `auki-p2p` at the pinned SDK revision with Android
enabled for its existing system-DNS adapter (see `vendor/auki-p2p/PATCH.md`).
This avoids Hickory's desktop `/etc/resolv.conf` initialization on Android and
uses the OS resolver without hardcoded public DNS servers. The ARM64 APK build
and six host resolver tests pass; authenticated startup on the phone still needs
a user sign-in retry. The mobile client now enables the SDK's default relay
allocation. Home must also enable relays and export a current connection profile
containing a relay route for networks that isolate clients. End-to-end relay
connectivity still requires a phone sign-in and successful scene receipt.

Install Java 17, the Rust `aarch64-linux-android` target, and Android SDK packages:
`platform-tools`, `platforms;android-35`, `build-tools;35.0.0`,
`ndk;27.2.12479018`, and `cmake;3.22.1`.

Set `JAVA_HOME` to the JDK and `ANDROID_HOME` to your SDK directory, then run:

```sh
sh android/build.sh
```

The script builds the shared Rust static library, packages its JNI adapter, and
runs Android lint and unit tests. Output: `android/app/build/outputs/apk/debug/app-debug.apk`.
The Gradle wrapper is pinned; toolchains, native build products, and local paths
are ignored by Git. The debug APK is for sideloaded development, not store release.
If unrelated work is in progress in the shared Rust tree, `SPATIAL_CORE_DIR` can
point to an isolated checkout of the same workspace. Record that checkout's commit
alongside the APK; the default builds from this repository.

Initial local validation used shared Rust core commit
`bf8593d5c5b0ab21f96695411bbb8515d324329b` from `.deps/android-core`,
to avoid mixing unrelated in-progress Rust edits into the APK. ARM64 cross-build,
APK packaging, three JVM unit tests, and lint passed (lint has advisory warnings).
APK signing and 16 KB native-library alignment checks passed. Hardware tests and
live Home connectivity remain unverified; the instrumentation APK is built for
that follow-up.

On 2026-10-05 the APK was rebuilt from the current working tree (not the isolated
core checkout): ARM64 Rust compilation, JNI/APK packaging, lint, and the JVM test
task passed. The connected USB device reported `unauthorized`, so installation,
device instrumentation, and real Home connectivity were not verified in this
pass. This does not advance the AR milestones above.

After USB authorization on the same date, the APK installed successfully on a
Pixel 7 Pro running Android 13. `connectedDebugAndroidTest` passed its one device
test: native-library loading and malformed-profile completion without leaking
the test password. Live connectivity remains unverified: the running Home at
port 4323 returned `Home P2P peer is not ready` from `/api/home-connection`.

## Device check

Enable USB debugging, connect the Pixel, and authorize the Mac. Install with:

```sh
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
```

With the phone connected, `cd android && ./gradlew connectedDebugAndroidTest`
also exercises loading the real JNI library, asynchronous Rust lifecycle, malformed
profile handling, and credential omission. It requires no live account.

Home must run with P2P enabled. Download `/api/home-connection` from Home to obtain
the public pairing profile, transfer it to the phone, and import it in the app.
The profile must advertise a route reachable from the phone; a loopback address
is not reachable. Use reachable direct routes or Home's relay routes; being on
the same Wi-Fi does not guarantee direct reachability. Sign in on the phone with an Auki User
account authorized for the selected Domain. Authentication currently uses the
same fixed development service environment as the iPhone annotator.

Passwords stay in memory and are cleared from the form on connect. The persistent
peer identity lives in Android's no-backup private directory. The imported public
profile is retained privately. Backgrounding requests shutdown; reconnection is
disabled until Rust reports cleanup complete. An orientation change also stops
the connection in this initial build.

Acceptance on hardware: valid login and scene receipt; rejected credentials;
malformed profile; Home restart; network loss/recovery; background cleanup;
repeat connection without duplicate peers. A compiled APK alone does not prove
authenticated connectivity to a real Home instance.
