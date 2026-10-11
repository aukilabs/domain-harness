# Humanoid Hub Conference

An independent scenegraph, not a new Auki transport Domain. Keep the existing
Home peer identity and authorization settings. Xmart and NACS remain separate.

Origin: Portal `HNY3JGNLRAE`, 0.15 m encoded-square side length, lying flat on
the floor. Its center is `(0, 0, 0)` in Home-space for this scenegraph. Its printed
face points +Y, right points +X, and top points -Z. The stored quaternion is a
-90 degree rotation about X, in `(w, x, y, z)` order. This defines horizontal
heading from the marker, not from compass north.

## Phone observation and alignment

Phone-space means the fixed tracked AR-session frame, not the moving camera
frame. From the same captured ARFrame, the phone measures `camera_from_portal`
using the 15 cm square and composes it with `phone_from_camera`:

`phone_from_portal = phone_from_camera * camera_from_portal`

The observed Portal does **not** have an identity pose in phone-space. Home's
reference Portal remains fixed; registration between the independent frames is:

`home_from_phone = home_from_portal * inverse(phone_from_portal)`

The inverse places Home content in the phone's AR view. Preserve observation
timestamps and session identities. A new AR session or lost tracking requires
fresh alignment; observations must never be relabeled with another scene's frame.

## Current integration status

Home seeds and exports the saved native SDK Map Product for this reference and
selects it in the Maps UI by default. Existing saved scenes are not overwritten.
The iPhone's capture-time pose composition already follows the equations above,
but its connection validation and Home contribution/annotation pipeline still
target the legacy Xmart reference. Scene-specific pairing and routing must be
completed before claiming the phone can annotate or contribute to this scene.
