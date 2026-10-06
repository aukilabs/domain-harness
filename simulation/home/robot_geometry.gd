extends RefCounted
## Shared collision/sensor dimensions, in meters. Sensors and their near
## planes must stay inside the full-height, rotation-invariant envelope.
const COLLISION_RADIUS := 0.4
const COLLISION_HEIGHT := 1.4
const CAMERA_MOUNT := Vector3(0, 1.15, -0.26)
const CAMERA_NEAR := 0.05
const CAMERA_FOV := 75.0
const LIDAR_MOUNT := Vector3(0, 0.65, -0.28)
