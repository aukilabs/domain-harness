# Semantic volume authoring

Scan a Home portal, aim at the calibrated floor, and choose **Create volume**.
One finger moves the draft on the floor, pinch changes its footprint, and a
two-finger rotation changes yaw. Width, height and depth fields accept metres.
Choose a name and arbitrary semantic type, then **Save / Retry**. Double-tap a
saved box to edit it; Delete requires confirmation.

This first version supports upright boxes, not pitch/roll or polygon extrusion.
Placement uses the Home floor (Y=0), not a newly detected ARKit plane. The ray
must point approximately 9 degrees downward and intersect within 10 m.
New dimensions start at one metre; accepted dimensions are 0.05–40 m.
Tracking loss, realignment, disconnect and backgrounding discard unsaved drafts.

Interaction patterns follow Peyote's native calibrated-floor placement and
local previews. This implementation does not import its Expo host, Cactus
persistence, shelf/configuration data, or scene revisions.

Home stores a typed volume collection alongside its scene annotations, isolated
by scene frame. The existing authenticated annotation operation accepts
put_volume and delete_volume; request-ID memoization makes immediate retries
idempotent. Publication exposes volumes separately from notes and voxels.
Scene-scoped mobile products keep the legacy text-note envelope unchanged for
older readers. Other clients may ignore the new volume layer.

Coordinates: bottom-centred position, local X/Y/Z dimensions, yaw around +Y,
all in Home's OpenGL metre frame. Home and iPhone render translucent boxes.
No synthetic voxels are generated. Native previews remain local until saved.

Device acceptance still required: calibrated placement, gestures, editing,
delete confirmation, lost tracking, disconnect/retry, and save/reopen after a
Home restart. Automated checks do not establish physical AR alignment or UX.

## Building and validation

From the repository root, with the iOS platform installed in Xcode:

```sh
TARGETS=aarch64-apple-ios sh ios/PortalBridge/build.sh
xcodegen generate --spec ios/PortalViewer/project.yml
open ios/PortalViewer/PortalViewer.xcodeproj
```

Select your signing team and iPhone in Xcode, then build/run Annotator. Build
the Rust bridge from this branch too; updating only Swift is insufficient.
The bridge script replaces its generated XCFramework, not phone data.

```sh
cargo test --locked --release --test semantic_volumes --test local_mobile --test mobile_notes --test note_changes
cargo test --locked --release --bin spatial-home lan_ui_tests
xcrun swiftc ios/PortalViewer/App/SemanticVolumeModel.swift ios/PortalViewer/Tests/Volumes/main.swift -o /tmp/semantic-volume-tests
/tmp/semantic-volume-tests
```

Home's release build, the targeted Rust tests, Swift model tests, and iOS Swift
typechecking passed locally. The device Rust bridge also built in release.
Full app packaging was blocked because Xcode reported the iOS 26.5 platform
component missing; no device installation or physical AR acceptance was done.
