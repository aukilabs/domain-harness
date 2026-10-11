# Domain Harness live demo

Status: planning draft. **Target: 2 minutes. Hard limit: 5 minutes.**

Keep all seven beats. Use one short line of narration per beat and switch views
as the presenter speaks. The extra three minutes are contingency for live
latency and one quick recovery, not additional planned content. Take questions
after the demo.

Show two Bracketbots building independent views of the same space, contributing
to one shared map, and exchanging spatial context with people using phones.
The audience should see a note placed on a phone appear in robot camera views,
then see a note placed through a robot camera appear on the phones.

## Demo setup

| Device | Role |
| --- | --- |
| Bracketbot `bot1` — `0184`, `192.168.1.118` | First camera and local map; camera used to place the return note. |
| Bracketbot `bot2` | Second camera and local map; complementary coverage of the space. |
| Phone 1 — presenter's phone | Joins during the demo; shows AR depth/geometry and robot labels; creates an annotation. |
| Phone 2 — second participant | Connected and aligned before the demo; independently confirms shared annotations. |
| One Home server | Receives mapping observations, builds the shared map, and serves the browser viewer and shared annotations. |

Use a presentation screen for the Home browser and a rehearsed way to show each
phone's camera view. Assign someone to switch the browser views while the
presenter handles Phone 1; the second participant holds Phone 2.

Use Portal **`HNY3JGNLRAE`**, with a **15 cm (0.15 m) encoded-square side length**,
as the planned shared spatial reference. Both robots should
observe it from different viewpoints and map partly overlapping areas. Choose a
distinctive surface that is visible from both robot cameras and reachable with
the phones. Use that surface for the annotations.

### Bot1 inspection — 11 October 2026

Connect with `bb ssh 0184`; the installed CLI resolves this ID to
`bracketbot@192.168.1.118`. The robot runs Ubuntu 22.04 on ARM64 with BBOS and
an installed Rust toolchain. BBOS exposes the stereo head camera, rectified
color, depth, colored points, and SLAM pose/health through shared memory.

The runner is implemented and deployed. Home received its calibrated
1280 × 960 head-camera stream and an independent map with 148 integrated
observations and 856 measured voxels during verification. It uses timestamp-matched
512 × 384 BBOS depth and capture-time SLAM poses, with explicit native frame axes.
Home reconnected after a controlled runner restart with the same peer identity.

Two hardware checks remain before rehearsal: BBOS tracking is intermittent and
was lost at the final check, so mapping paused while camera publication continued;
the visible floor Portal has not decoded at its current image size/blur. The
presenter asked to retain its current position. Shared conference-map alignment
and phone/robot annotations are not yet verified. See
[Bracketbot setup and verification](bracketbot-runner.md).

## Before the audience arrives

- [ ] Assign owners for presenting, browser operation, and Phone 2.
- [ ] Deploy the Bracketbot runner to bot2 with its own identity; bot1's camera,
      local mapping, and Home transport have been verified. Resolve bot1's
      tracking stability and demonstrate live Portal decoding on both robots.
- [ ] Start one Home server and both robot peers; use clear `bot1` / `bot2`
      display names and distinct persistent peer identities.
- [ ] Check each robot's live camera and independent map in Home.
- [ ] Confirm the shared reference's identity, measured size, and physical pose
      agree with the configuration. Verify alignment and contributions from both
      robots in the shared Home map.
- [ ] Pick the camera on each robot that has calibrated AR and sees the annotation
      surface. Verify that surface has live mapped geometry for note placement.
- [ ] Prepare two supported iPhones with the native app installed. Phone 1 needs
      LiDAR support for the depth-sharing part of this script.
- [ ] Import Home's current connection profile on both phones, verify authorized
      sign-in and network reachability, and grant camera/local-network permissions.
- [ ] Rehearse Phone 1's join, Portal alignment, depth sharing, and annotation save.
      Prepare its profile and sign-in fields, and leave the app foregrounded at
      the connection screen for step 4. Connection and alignment happen live;
      profile import and permission dialogs happen before the demo.
- [ ] Keep Phone 2 connected, aligned, and foregrounded. Verify the phones can
      receive both phone-authored and Home-authored notes.
- [ ] Confirm robot labels appear correctly in phone AR and spatial notes appear
      in both robot camera overlays.
- [ ] Rehearse right-click placement through `bot1`'s calibrated camera image.
- [ ] Pre-open Home with `bot1`'s camera and map framed. Rehearse every view switch
      and phone-screen handoff; keep the reference and annotation surface in reach.
- [ ] Prepare the two short note texts for quick entry: **“Keys live here”** and
      **“Check here”**. Create and save both annotations live.
- [ ] Remove previous rehearsal notes and check phone screen presentation,
      batteries, lighting, and network stability.

The live demo starts with mapping and robot alignment already established.
Step 3 reveals the shared map. Keep setup explanations and first-time robot
alignment outside the timed sequence.

## Two-minute live sequence

| Time | Beat | Audience sees |
| --- | --- | --- |
| 0:00–0:10 | 1. Bot1 | Its live camera and independent map. |
| 0:10–0:20 | 2. Bot2 | Another camera perspective and independent map. |
| 0:20–0:35 | 3. Shared reference → shared map | Both robots and their combined mapping contributions. |
| 0:35–1:00 | 4. Phone 1 joins | Live connection, Portal scan, AR geometry and robot labels. |
| 1:00–1:20 | 5. Phone annotation | “Keys live here” placed and saved at the staged surface. |
| 1:20–1:35 | 6. Robot views receive it | The same note in both robot camera overlays. |
| 1:35–2:00 | 7. Note back to phones + close | “Check here” placed through bot1's camera and visible on both phones. |

### 1. Bot1 — 10 seconds

**Do:** Start on the prepared Home view with `bot1`'s camera and independent map.

**Say:** “Here's bot1's live camera and the map it's building.”

### 2. Bot2 — 10 seconds

**Do:** Switch both map and camera to `bot2`.

**Say:** “Bot2 sees the same room from a different angle and builds its own map.”

### 3. Shared reference → shared map — 15 seconds

**Do:** Point out the shared Portal, then switch to the Home live map with both
robots and their contributions in frame.

**Say:** “They've spotted the same reference, so Home can align their observations
into one shared map.”

### 4. Phone 1 joins — 25 seconds

**Do:** Show Phone 1, connect using the prepared sign-in screen, and scan
`HNY3JGNLRAE`. Pan just enough to show AR depth/geometry and robot labels. Keep
**Share LiDAR depth** enabled. Phone 2 stays connected as the second participant;
its screen reveal comes in the final beat.

**Say:** “People can join too. My phone joins the same space and sees the mapped
surfaces and robot labels.”

### 5. Phone annotation — 20 seconds

**Do:** Double-tap the staged surface, check the preview, enter **“Keys live here”**,
and save. Move the camera slightly to show the note stays in place. Phone 2's
operator checks receipt while the presentation continues.

**Say:** “I'll leave a note at this spot: ‘Keys live here.’”

### 6. Both robot views receive it — 15 seconds

**Do:** Return to Home with the shared map selected. Show the note through `bot1`'s
camera with **AR** enabled, then switch to `bot2`'s camera and show the same note.

**Say:** “The note is now visible from both robots' viewpoints.”

### 7. Note back to the phones and close — 25 seconds

**Do:** Return to `bot1`'s calibrated camera, keeping the shared Home map selected.
Right-click the rehearsed surface, enter **“Check here”**, and save. Show the
result on Phone 1, then briefly reveal Phone 2 displaying the same note.

**Say:** “Through bot1's camera, I can leave a note for the people here. Both phones
see it. Two robots, two phones, one shared space.”

## Rehearsal and the five-minute limit

- Record the second Bracketbot's address, camera selections, annotation surface,
  and operator names after the first hardware rehearsal.
- Time the complete sequence, including connection, view switches, typing,
  propagation, and the closing line. Aim to finish by **2:00**. Trim narration and
  handoffs first; retain all seven beats and both directions of annotation sharing.
- At **1:00**, aim to have Phone 1 aligned and ready to place its note. At **1:35**,
  aim to begin the return note through bot1. Treat overruns as use of contingency.
- Allow one quick recovery for a stalled feed, missed surface, or lost phone
  alignment. Cap active troubleshooting at **10 seconds**, then move to a working
  view or a prepared recording. Identify a recording explicitly and use only a
  clip that fits within the remaining time.
- At **4:30**, wrap up using the available views. State any proof step that could
  not be shown. End the demo by **5:00**, including recovery and the closing line.

## Acceptance checklist

- [ ] Full rehearsal targets 2:00; the demo ends by 5:00 including any recovery.
- [ ] Both Bracketbots provide live camera views and independent measured maps.
- [ ] Home's shared map contains aligned contributions from both robots.
- [ ] Phone 1 joins live and shows mapped geometry and robot labels in AR.
- [ ] Both phones participate and receive shared annotations.
- [ ] A Phone 1 annotation appears in both robot camera AR views.
- [ ] A note placed through bot1's camera appears on both phones.

## Implementation references

- [Home viewer, shared mapping, and camera annotation behavior](home.md)
- [Robot runner and hardware integration](runner.md)
- [iPhone connection, AR alignment, depth sharing, and annotations](ios/PortalViewer/README.md)
