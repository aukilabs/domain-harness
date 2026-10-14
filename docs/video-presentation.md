# PiP orientation is a presentation transform

Home does not rotate the transmitted image, its intrinsics, its frame registry,
or its pose. RGB and the AR canvas share one rotated display layer. Camera
picks invert that rotation before sending native image pixels to raycasting.
The window's aspect ratio follows the rotated image; resizing never crops it.

Automatic orientation, independent of peer names and device models:

1. Honor an optional camera Product metadata `presentation` declaration:
   `{"schema":"spatial.video-presentation/v1","rotation_clockwise_deg":90}`.
   This angle is a display correction for the transmitted pixels, NOT the
   existing `rotation_from_camera_frame_deg` (which records image preprocessing).
2. Otherwise, find an unambiguous compact sampled camera pose with the same
   source frame reference and capture clock. Fetch its registry definitions and
   exact/bracketed capture-time pose; never use arrival time or a latest pose.
   Project the target frame's declared up direction into the camera, convert
   conventions with auki-geometry, account for any declared image preprocessing,
   and choose the nearest upright quarter turn. This policy uses the sampled
   pose's destination as the upright reference; gravity-aligned AR/odometry
   frames provide the intended behavior.
3. Without a usable pose/declaration (including lost tracking, missing history,
   or looking almost directly up/down), preserve the last display angle. A new
   feed starts at zero. Never infer portrait from a peer name or image dimensions.

The PiP orientation selector offers Auto and four explicit quarter turns.
Overrides are saved locally per peer/Product, not sent to the publisher.
Thus video without pose data remains usable and can be oriented manually.
Explicit image-display declarations can cover mounts that lack camera poses.

Orientation lookup is bounded to 200 ms and cached per received frame. Failed
lookups do not block video. Subsequent automatic angle changes require two
matching frames to reduce jitter at quarter-turn boundaries. This is display
normalization, not continuous horizon stabilization.
