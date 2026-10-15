# Robot runners

Bracketbot is the supported physical robot integration in this repository.
See [bracketbot-runner.md](bracketbot-runner.md) for the main runner and
[Diagnostics](diagnostics/connection-peer/README.md) for the registry-backed
diagnostics runner. Use one runner at a time; building is not deployment.
Both use SDK Components and authenticated libp2p peer communication.

## Semantic volume speech (Bracketbot)

### Bracketbot pose-graph correction handling

Bracket uses capture-time geometry: ordinary BBOS pose-graph updates are accepted
as live poses in the same robot frame. Capture never reads optimized trajectory
history, suppresses odometry for PGO, or back-propagates corrections into old
points or portal measurements. Geometry may drift or have discontinuities; this
is intentional. No raw depth is retained for correction replay on Bracket.

Portal localization produces a separate robot-to-Home relationship. Home retains
the last validated relationship per authenticated producer, native frame and
capture clock/session, and applies it to new native observations. Neither mapper
revisits old samples. Robot presence always carries a native base pose; Home
composes it for labels. Expired pose samples do not expire the relationship.

Incoming notes, phone poses and volume collections declare their coordinate
frame. Consumers convert between the named robot and Home frames using the last
known relationship, or reject unknown frames. Missing/stale live poses still
pause actions; this is separate from the validity of a saved frame relationship.

Producer restart, clock regression and a decreasing PGO counter require a new
capture session and fresh calibration. After an ordinary runner restart, show a
known portal once before driving away.

The previous history-replay implementation caused mapping pauses despite healthy
BBOS tracking. A stationary 60-second runner-off comparison reduced median total
CPU usage from 97.8% to 66.0%; restoring the runner raised it to 94.5%. This
establishes overhead, not causation of actual SLAM tracking loss.

The 2026-10-12 fix was built on the laptop with cached Linux ARM64 Docker:
`build-bracketbot-correction.py`, then transferred/compatibility-checked with
`install-bracketbot-correction.py`. The installer refuses to overwrite newer
deployed sources. `restart-bracketbot-presence.py` keeps the existing supervisor
arguments/environment. Home and the BBOS SLAM daemon are not rebuilt/restarted.
Tests verify that corrected calibration survives with no new marker observation
and post-correction depth scans remain available for contributions. Live driving
verification still requires a fresh portal observation after deployment.

### Presence and speech

Robot name labels use a separate authenticated `home-robot-presence:<scene>`
operation, independent of geometry contribution acceptance. The runner supplies
its native `robot.base` pose and mapper-owned Home transform; Home composes them
without rebasing the robot. Presence expires after three seconds and is withdrawn
when tracking/alignment is unavailable. Robot labels are read-only and separate
from phone presence, so robots never become phone-reaching targets. Pose
publication diagnostics are written to `robot-presence-status.json`.

The pose path passes local authenticated integration tests, including publication
without any voxel geometry. The runner-only update was installed on Bracketbot
0184 on 2026-10-12 using `build-bracketbot-presence.py`; unrelated deployed
sources were preserved. `restart-bracketbot-presence.py` restarted its existing
supervisor with identical arguments/environment, retaining phone driving. Home
was not rebuilt or restarted; its receiver must be deployed separately. Current
pose diagnostics report the receiver unavailable and the mapper alignment stale.

The supervisor starts `speak-bracketbot-volumes.py` by default. The runner
subscribes to the authenticated Home mobile scene selected by
`home-connection.json` and writes only its semantic-volume extension to
`semantic-volumes.json`. This currently uses the shared scene Product (including
its geometry on the wire), not a dedicated lightweight volume Product.

The speech controller transforms the live **robot.base** position into Home
space without changing the robot's native frame. It reads each nonblank
description after a 0.5-second entry dwell. Boxes honor yaw and height; overlapping
volumes are announced in stable ID order. A 5 cm exit margin suppresses boundary
chatter. Lost/stale localization pauses announcements and is not treated as an
exit. Editing a description while already inside does not repeat it.

Offline synthesis requires `espeak-ng` (or `espeak`) on the robot. Playback uses
the existing BBOS `speaker.audio` topic and speaker daemon's Python environment,
including its NumPy and soxr dependencies, without claiming the ALSA device. The speech
worker reports backend/playback errors in `volume-speech-status.json` and does
not block SLAM, change audio services, or command motors. Failed announcements
are not retried continuously. Use supervisor `--disable-volume-speech` to opt out.

Verified on Bracketbot 0184 on 2026-10-12: ARM64 build/install, offline synthesis,
BBOS speaker playback smoke test, and authenticated ingestion of the Home
`Office` volume. The launched runner resumed mapping but its Home alignment
remained stale, so a real localized volume-entry announcement remains unverified.
