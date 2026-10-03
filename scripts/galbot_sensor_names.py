"""Read-only camera enum and optical-frame names from the original Galbot adapter."""
from galbot_sdk.g1 import SensorType

galbot_sensor_types = {
    "left_arm": SensorType.LEFT_ARM_CAMERA,
    "right_arm": SensorType.RIGHT_ARM_CAMERA,
    "head_left": SensorType.HEAD_LEFT_CAMERA,
    "head_right": SensorType.HEAD_RIGHT_CAMERA,
    "front_left": SensorType.LEFT_FRONT_SURROUND_CAMERA,
    "front_right": SensorType.RIGHT_FRONT_SURROUND_CAMERA,
    "rear_left": SensorType.LEFT_REAR_SURROUND_CAMERA,
    "rear_right": SensorType.RIGHT_REAR_SURROUND_CAMERA
}

galbot_sensor_link_names = {
    "left_arm": "left_arm_camera_color_optical_frame",
    "right_arm": "right_arm_camera_color_optical_frame",
    "head_left": "head_left_camera_color_optical_frame",
    "head_right": "head_right_camera_color_optical_frame",
    "front_left": "left_front_surround_color_optical_frame",
    "front_right": "right_front_surround_color_optical_frame",
    "rear_left": "left_rear_surround_color_optical_frame",
    "rear_right": "right_rear_surround_color_optical_frame"
}
