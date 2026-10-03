"""GalbotSDK wrapper -- the single robot-facing surface for this app.

Covers cameras, depth, arms/IK, head, torso/leg, base, navigation and
obstacles, plus three distinct base-velocity paths (see the `base` section).
Changes made when this was consolidated, all deliberate:

1. IMPORT FAILURE RAISES, it does not exit(1). The original killed the
   process on a missing galbot_sdk with a one-line print, which meant this
   module -- and therefore anything importing it -- could not even be loaded on
   a machine without the robot SDK. Now `main.py` catches it, reports it on
   /api/status, and keeps the HTTP surface up.

2. CONSTRUCTION NO LONGER MOVES THE ROBOT. `__init__` used to call
   `stand_qr_home()`, so merely starting the app physically raised the torso.
   Homing is now the explicit `home()` method, opt-in at boot via
   GALBOT_HOME_ON_START, otherwise POST /api/home.

3. Every `exit(1)` in `__init__` raises `GalbotInitError` instead.

4. Two clearly-named velocity paths (see `set_base_velocity` /
   `set_base_velocity_raw`) instead of one whose collision-checking behaviour
   silently differed between the two source repos.

5. Perception (Foundation-Stereo) is optional -- it is the slowest part of
   startup and only `get_depth_image("head_left")` needs it.

6. `cam_pose_wrt_ee` is lazy: it was a 5s TF lookup on the construction path.
"""

try:
    from galbot_sdk.g1 import (
        ControlStatus,
        G1ControllerName,
        GalbotMotion,
        GalbotNavigation,
        GalbotPerception,
        GalbotRobot,
        JointCommand,
        JointStates,
        MachineType,
        MotionStatus,
        NavigationTaskStatus,
        Parameter,
        PerceptionModule,
        Pose,
        PoseState,
        RgbData,
        SensorType,
        Trajectory,
        TrajectoryPoint,
    )
except ImportError as _exc:  # pragma: no cover - depends on the robot image
    raise ImportError(
        "import galbot_sdk failed -- install it or add it to PYTHONPATH. "
        "This module is robot-only; utils.transforms / utils.waypoints / "
        "utils.esl and config.py import fine without it."
    ) from _exc

import cv2
import time
import numpy as np
from typing import List

from utils import nav_tuning
from utils.waypoints import pose_reached
from utils.robot_params import (
    FALLBACK_DEFAULTS,
    load_defaults,
    load_effective_params,
    validate_params,
)
from utils.torso_bands import (
    TORSO_EE_CEILING_M,
    TORSO_EE_FLOOR_M,
    TORSO_NEAR_EPS_M,
)


class GalbotError(RuntimeError):
    """Base for robot-control failures raised by this module."""


class GalbotInitError(GalbotError):
    """A subsystem failed to initialize. Replaces the old exit(1) calls."""


# NOTE: this dict was defined twice, identically, back to back. One copy.
chain_ee_frames = {
    "leg": "leg_end_effector_mount_link",
    "head": "head_end_effector_mount_link",
    "left_arm": "left_arm_end_effector_mount_link",
    "right_arm": "right_arm_end_effector_mount_link"
}

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

nav_status_str = {
    NavigationTaskStatus.CLOSE_TO_OBSTACLE: "close_to_obstacle",
    NavigationTaskStatus.COLLISION: "collision",
    NavigationTaskStatus.FAILED: "failed",
    NavigationTaskStatus.INTERRUPTED: "interrupted",
    NavigationTaskStatus.OCCUPIED: "occupied",
    NavigationTaskStatus.RUNNING: "running",
    NavigationTaskStatus.SUCCESS: "success",
    NavigationTaskStatus.UNKNOWN: "unknown",
}

from utils.logs import get_logger

log = get_logger(__name__)


def generate_target_point(q: List[float], target_time: float = 10):
    """Generate target for joints"""
    joint_position = TrajectoryPoint()
    joint_position.time_from_start_second = target_time
    joint_command_vec = []
    for joint in q:
        joint_cmd = JointCommand()
        joint_cmd.position = joint
        joint_command_vec.append(joint_cmd)
    joint_position.joint_command_vec = joint_command_vec
    return joint_position

def decode_rgb_image(image_data):
    """decode rgb image"""
    nparr = np.frombuffer(image_data, np.uint8)
    img = cv2.imdecode(nparr, cv2.IMREAD_COLOR)
    if img is None:
        raise ValueError("Fail to Decode RGB Image")
    return img

def decode_depth_image(image_data):
    """decode depth image"""
    depth_img = np.frombuffer(image_data["data"], dtype=np.uint16).copy()

    # check if height and width exist
    if "height" not in image_data or "width" not in image_data:
        raise ValueError("Missing 'height' or 'width' in depth image metadata.")
    if image_data["height"] == 0 or image_data["width"] == 0:
        raise ValueError(f"Invalid 'height' ({image_data['height']}) or 'width' ({image_data['width']}) in depth image metadata.")

    # parse depth image
    depth_img = depth_img.reshape((image_data["height"], image_data["width"]))
    depth_img = depth_img.astype(np.float32) / image_data["depth_scale"]

    return depth_img

def decode_compressed_image(compressed_image):
    """
    decode CompressedImage image

    Parameters:
        compressed_image (dict): image dict, keys:[header, format, data, "depth_scale"]

    Returns:
        numpy.ndarray: decoded image
    """
    image_data = compressed_image["data"]
    if compressed_image["format"].lower() in ("rgb8", "jpeg", "jpg"):
        return decode_rgb_image(image_data)
    elif compressed_image["format"] == "16UC1":
        return decode_depth_image(compressed_image)
    else:
        raise ValueError(f"Unsupport data format: {compressed_image['format']}")

def decode_compressed_RgbData(rgb_data: RgbData):
    """
    decode CompressedImage image

    Parameters:
        compressed_image (dict): image dict, keys:[header, format, data, "depth_scale"]

    Returns:
        numpy.ndarray: decoded image
    """
    image_data = rgb_data.data
    if rgb_data.format.lower() in ("rgb8", "jpeg", "jpg"):
        return decode_rgb_image(image_data)
    else:
        raise ValueError(f"Unsupport data format: {rgb_data.format}")

# Full sensor set enabled by default. Override with GALBOT_SENSORS (a
# comma-separated list of galbot_sensor_types keys) to cut startup cost when a
# deployment only needs some cameras.
DEFAULT_SENSOR_SET = (
    "left_arm", "left_arm_depth", "right_arm", "right_arm_depth",
    "head_left", "head_right", "front_left", "front_right", "rear_left", "rear_right",
    "base_lidar", "chassis_imu", "base_ultrasonic",
)

_SENSOR_ENUMS = {
    "base_lidar": "BASE_LIDAR",
    "chassis_imu": "CHASSIS_IMU",
    "base_ultrasonic": "BASE_ULTRASONIC",
    "left_arm": "LEFT_ARM_CAMERA",
    "left_arm_depth": "LEFT_ARM_DEPTH_CAMERA",
    "right_arm": "RIGHT_ARM_CAMERA",
    "right_arm_depth": "RIGHT_ARM_DEPTH_CAMERA",
    "head_left": "HEAD_LEFT_CAMERA",
    "head_right": "HEAD_RIGHT_CAMERA",
    "front_left": "LEFT_FRONT_SURROUND_CAMERA",
    "front_right": "RIGHT_FRONT_SURROUND_CAMERA",
    "rear_left": "LEFT_REAR_SURROUND_CAMERA",
    "rear_right": "RIGHT_REAR_SURROUND_CAMERA",
}


class GalbotControl():
    """Owns the one process-wide GalbotSDK session.

    Constructing this does NOT move the robot -- call `home()` for that.
    Raises GalbotInitError if a subsystem fails, so the caller can report the
    failure over HTTP instead of the process dying.
    """

    def __init__(self, debug=False, sensors=None, enable_motion=True,
                 enable_navigation=True, enable_perception=True,
                 apply_speed_params=True, init_settle_s=3.0,
                 home_on_init=False):
        log.info("Initializing GalbotControl...")
        self.debug = debug
        self.obstacle_service_ok = False
        self.perception = None
        self.navigation = None
        self.motion = None
        self._cam_pose_wrt_ee = None

        self.robot = GalbotRobot()
        enable_sensor_set = self._resolve_sensor_set(sensors)
        # Second arg is sync mode -- required for get_synced_observation(),
        # which the calibration loop and every Auki capture path rely on.
        if self.robot.init(enable_sensor_set, True):
            log.info("GalbotRobot initialization successful")
        else:
            raise GalbotInitError("GalbotRobot.init failed")

        # A controller-mode set, not motion: safe in a constructor, and every
        # navigation path below assumes it.
        self.robot.switch_controller(G1ControllerName.CHASSIS_POSE_CTRL)

        if enable_motion:
            self.motion = GalbotMotion()
            if self.motion.init():
                log.info("GalbotMotion initialization successful")
            else:
                raise GalbotInitError("GalbotMotion.init failed")

            # Add-then-clear primes the obstacle service. A dead obstacle
            # service used to exit(1); it must not, because pose/joint
            # publishing and teleop are all still perfectly usable without it.
            # Arm tasks check `obstacle_service_ok` and refuse rather than
            # planning into a shelf with no obstacle registered.
            if self.add_box_obstacle(
                    obstacle_id="empty",
                    obstacle_size=[1.0, 1.0, 1.0],
                    obstacle_pose=[10.0, 10.0, 10.0, 0.0, 0.0, 0.0, 1.0],
                    target_frame='base_link') and self.clear_obstacle():
                self.obstacle_service_ok = True
            else:
                log.warning("WARNING: obstacle service handshake failed; arm tasks "
                         "will refuse to run (teleop, pose and joint publishing "
                         "are unaffected)")

        if enable_navigation:
            self.navigation = GalbotNavigation()
            if self.navigation.init():
                log.info("GalbotNavigation initialization successful")
            else:
                raise GalbotInitError("GalbotNavigation.init failed")

            ret, configs = self.navigation.dump_navigation_configs()
            log.info(f"navigation configs: \n{configs}")

        # Foundation-Stereo is the slowest step in startup (init + settle) and
        # is used ONLY by get_depth_image("head_left"), i.e. ESL detection and
        # product inspect. Skippable for a teleop/capture-only deployment.
        if enable_perception:
            self.perception = GalbotPerception.get_instance(MachineType.G1)
            if self.perception.init({PerceptionModule.FOUNDATION_STEREO}):
                log.info("GalbotPerception initialization successful")
            else:
                raise GalbotInitError("GalbotPerception.init failed")
            time.sleep(init_settle_s)
        else:
            log.info("GalbotPerception disabled; get_depth_image('head_left') "
                     "will raise")

        # Head joint speed stays 0.3 rad/s (not a panel knob).
        # Arm/leg/base come from robot_params.yaml (+ overlay).
        self.max_speed_rad_s = 0.3
        self.max_speed = 0.05
        self.timeout_s = 30.0

        if apply_speed_params:
            speeds, err = validate_params(load_effective_params())
            if speeds is None:
                log.warning(f"robot_params invalid ({err}); using yaml defaults")
                speeds, err = validate_params(load_defaults())
            if speeds is None:
                log.warning(f"robot_params defaults invalid ({err}); using fallback")
                speeds = dict(FALLBACK_DEFAULTS)
            ret, status = self.apply_robot_speeds(speeds)
            if not ret:
                log.warning(f"Failed to apply robot speeds, status: {status}")

        # `home_on_init` PHYSICALLY MOVES THE ROBOT (raises the torso to the QR
        # home pose). It defaults to False: the pre-merge constructor always did
        # this, so importing and constructing the SDK moved the robot.
        if home_on_init:
            if not self.home():
                raise GalbotInitError("home() failed during init")

        log.info("GalbotControl initialized successfully.")

    @staticmethod
    def _resolve_sensor_set(sensors=None):
        """Build the SensorType set from key names, defaulting to all of them."""
        keys = tuple(sensors) if sensors else DEFAULT_SENSOR_SET
        out = set()
        for key in keys:
            enum_name = _SENSOR_ENUMS.get(key)
            if enum_name is None:
                raise GalbotInitError(
                    f"unknown sensor key {key!r}; valid: {sorted(_SENSOR_ENUMS)}"
                )
            out.add(getattr(SensorType, enum_name))
        return out

    @property
    def cam_pose_wrt_ee(self):
        """Wrist camera pose relative to the arm EE mount, resolved on first use.

        Lazy because this is a TF lookup with a multi-second timeout, and it used
        to sit on the construction path where it delayed every startup -- including
        startups that never touch the arms.
        """
        if self._cam_pose_wrt_ee is None:
            self._cam_pose_wrt_ee = self.get_relative_pose(
                "left_arm_camera_color_optical_frame",
                "left_arm_end_effector_mount_link",
            )
        return self._cam_pose_wrt_ee

    def home(self):
        """Move to the standing QR home pose. THIS MOVES THE ROBOT."""
        self.switch_pose_ctrl()
        return bool(self.stand_qr_home())

    def switch_pose_ctrl(self):
        self.robot.switch_controller(G1ControllerName.CHASSIS_POSE_CTRL)
        time.sleep(1.0)
        return

    def get_all_joint_poses(self):
        joint_names = self.robot.get_joint_names(True, [])
        states = self.robot.get_joint_states([], joint_names)
        joint_poses = {}
        if len(joint_names) != len(states):
            return joint_poses
        for i, joint_name in enumerate(joint_names):
            joint_poses[joint_name] = states[i].position
        return joint_poses

    def get_arm_camera_intrinsics_matrix(self, side='left'):
        """Wrist camera K as a 3x3.

        FIXED: this used to read self.left_arm_camera_intrinsics_matrix /
        self.right_arm_camera_intrinsics_matrix, neither of which is ever
        assigned anywhere -- so any call raised AttributeError. Delegates to
        the live getter instead.

        Returns a FRESH array each call. Do not add caching here: the
        auki_calibration DomainCalibrator scales a caller-supplied camera
        matrix IN PLACE (it assigns without copying, then does `*= scale`), so
        a shared/cached matrix would be corrupted a little more on every frame
        and calibration would quietly drift.
        """
        return self.get_cam_intrinsics(f"{'left' if side == 'left' else 'right'}_arm")

    def leg_needs_raise(self):
        """True if leg joints are not near standing QR home (need raise from LOW)."""
        leg_target = [0.32, 1.14, 0.82, 0.0, 0.0]
        try:
            rs = self.motion.get_robot_states()
            wb = list(getattr(rs, "whole_body_joint", None) or [])
            if len(wb) < 5:
                return True
            cur_leg = [float(v) for v in wb[:5]]
            return any(abs(a - b) > 0.05 for a, b in zip(cur_leg, leg_target))
        except Exception as e:
            log.warning(f"leg_needs_raise: state check failed ({e}); assuming raise")
            return True

    # Single source of truth: utils/torso_bands.py, imported at module top and
    # shared with utils/torso.py. These aliases keep the existing `self._TORSO_*`
    # references below working.
    _TORSO_EE_FLOOR_M = TORSO_EE_FLOOR_M
    _TORSO_EE_CEILING_M = TORSO_EE_CEILING_M
    _TORSO_NEAR_EPS_M = TORSO_NEAR_EPS_M

    def torso_repark_band(self):
        """low_floor | mid_upright | high_fixed from live torso EE z."""
        pose = self.get_torso_pose()
        if pose is None or len(pose) < 3:
            log.info("torso_repark_band: no torso pose; defaulting to mid_upright")
            return "mid_upright"
        z = float(pose[2])
        if z <= self._TORSO_EE_FLOOR_M + self._TORSO_NEAR_EPS_M:
            return "low_floor"
        if z >= self._TORSO_EE_CEILING_M - self._TORSO_NEAR_EPS_M:
            return "high_fixed"
        return "mid_upright"

    def stand_qr_home(self):
        """Band-aware standing QR home (init + repark).

        low_floor: low_operation_tuck → raise leg (no second tuck) → QR.
        mid / high / standing: reset_arm_pose → raise leg if needed (no low tuck) → QR.
        """
        needs_raise = self.leg_needs_raise()
        band = self.torso_repark_band()
        log.info(f"stand_qr_home band={band} leg_needs_raise={needs_raise}")
        ok_prep = True
        if not needs_raise or band != "low_floor":
            if not self.reset_arm_pose():
                log.warning("stand_qr_home: reset_arm_pose failed; continuing")
                ok_prep = False
        else:
            if not self.low_operation_tuck(verify_height=False):
                log.warning("stand_qr_home: low_operation_tuck failed; continuing")
                ok_prep = False
        ok = self.set_init_pose(pre_raise_tuck=False)
        return bool(ok_prep and ok)

    _QR_HOME_LEG = [0.32, 1.14, 0.82, 0.0, 0.0]
    _QR_HOME_HEAD = [0.0, 0.37]
    _QR_HOME_LEFT = [1.7, -1.5, -0.57, -1.5, 0.0, 0.0, 0.0]
    _QR_HOME_RIGHT = [-1.7, 1.5, 0.57, 1.5, 0.0, 0.0, 0.0]
    # 0.10 covers ESL survey head [0.0, 0.3] vs QR [0.0, 0.37] (Δ≈0.07);
    # tighter 0.05 caused near-noop head/arm execute TIMEOUT after repark.
    _QR_HOME_TOL_RAD = 0.10

    def _joints_near(self, current, target, tol=None):
        tol = self._QR_HOME_TOL_RAD if tol is None else tol
        if current is None or len(current) < len(target):
            return False
        return all(abs(float(a) - float(b)) <= tol for a, b in zip(current, target))

    def _wb_leg_head_arms(self):
        """Return (leg[5], head[2], left[7], right[7]) from whole_body_joint, or Nones."""
        try:
            rs = self.motion.get_robot_states()
            wb = list(getattr(rs, "whole_body_joint", None) or [])
        except Exception:
            return None, None, None, None
        if len(wb) < 21:
            return None, None, None, None
        return (
            [float(v) for v in wb[0:5]],
            [float(v) for v in wb[5:7]],
            [float(v) for v in wb[7:14]],
            [float(v) for v in wb[14:21]],
        )

    def set_init_pose(self, pre_raise_tuck=True):
        """Restore standing QR home.

        From LOW + inspect aim, leg-only move_line FAULTs: standing end-state
        collides (leg_link2 vs right_arm_link4). Proven recovery: low-op tuck
        while still LOW, then raise leg, then QR head/arms.

        Mid repark must NOT low-tuck then raise: that execute FAULTs with arms
        left in low tuck. Caller should QR (`reset_arm_pose`) at mid first and
        pass ``pre_raise_tuck=False``.

        Skip plan/execute for chains already within home tolerance — Singorix
        can TIMEOUT on near-noop trajectories that the planner still returns
        as non-empty (seen 2026-08-10).
        """
        needs_raise = self.leg_needs_raise()

        # Phase 0: low-op tuck only when raising from LOW (caller may already
        # have tucked, or mid/high path uses reset_arm_pose instead).
        if needs_raise and pre_raise_tuck:
            if not self.low_operation_tuck(verify_height=False):
                log.error("❌ set_init_pose: low_operation_tuck before repark failed")
                return False

        # Phase 1: leg home — skip if already standing.
        if needs_raise:
            leg_traj = self.plan_traj_from_joint_positions(self._QR_HOME_LEG, "leg")
            if leg_traj is None:
                log.error("❌ set_init_pose: leg trajectory planning failed")
                return False
            if not self.execute_trajectories({"leg": leg_traj}):
                log.error("❌ set_init_pose: leg execute failed")
                return False
        else:
            log.info("set_init_pose: leg already at home; skipping leg execute")

        # Phase 2: head + arms (QR downward tuck) — skip chains already home.
        leg, head, left, right = self._wb_leg_head_arms()
        chain_joints = {}
        if not self._joints_near(head, self._QR_HOME_HEAD):
            chain_joints["head"] = self._QR_HOME_HEAD
        if not self._joints_near(left, self._QR_HOME_LEFT):
            chain_joints["left_arm"] = self._QR_HOME_LEFT
        if not self._joints_near(right, self._QR_HOME_RIGHT):
            chain_joints["right_arm"] = self._QR_HOME_RIGHT
        if not chain_joints:
            log.info("set_init_pose: head/arms already at QR home; skipping")
            return True

        trajs = {}
        for chain_name, joint_angles in chain_joints.items():
            trajs[chain_name] = self.plan_traj_from_joint_positions(joint_angles, chain_name)
        if any(t is None for t in trajs.values()):
            log.error("❌ set_init_pose: head/arm trajectory planning failed")
            return False
        if not self.execute_trajectories(trajs):
            log.error("❌ set_init_pose: head/arm execute failed")
            return False
        return True

    def reset_arm_pose(self):
        """QR-scan home tuck (wrists aimed down). Not safe before lowering the leg."""
        _, _, left, right = self._wb_leg_head_arms()
        chain_joints = {}
        if not self._joints_near(left, self._QR_HOME_LEFT):
            chain_joints["left_arm"] = self._QR_HOME_LEFT
        if not self._joints_near(right, self._QR_HOME_RIGHT):
            chain_joints["right_arm"] = self._QR_HOME_RIGHT
        if not chain_joints:
            log.info("reset_arm_pose: arms already at QR home; skipping")
            return True
        trajs = {}
        for chain_name, joint_angles in chain_joints.items():
            trajs[chain_name] = self.plan_traj_from_joint_positions(joint_angles, chain_name)
        if any(t is None for t in trajs.values()):
            log.error("❌ reset_arm_pose: trajectory planning failed")
            return False
        return self.execute_trajectories(trajs)

    # Low-operation tuck: taught from live left arm (2026-08-04), right mirrored.
    # Height check uses post-fold LOW torso band (not standing leg_link5 — taught
    # elbows sit below standing j4/5 but clear the column after it folds).
    _LOW_OPERATION_TUCK_LEFT = [1.6847, -1.5010, -0.5675, -2.2255, 0.0, 0.0, 0.0]
    _LOW_OPERATION_TUCK_RIGHT = [-1.6847, 1.5010, 0.5675, 2.2255, 0.0, 0.0, 0.0]
    _LOW_OP_TUCK_ELBOW = {
        "left_arm": "left_arm_link4",
        "right_arm": "right_arm_link4",
    }
    _LOW_OP_TUCK_WRIST = {
        "left_arm": "left_arm_end_effector_mount_link",
        "right_arm": "right_arm_end_effector_mount_link",
    }
    # Keep in sync with tasks._TORSO_LOW_Z_M — arms must clear folded column height.
    _LOW_OP_TUCK_MIN_Z_M = 0.55
    _LOW_OP_TUCK_Z_EPS_M = 0.02

    def low_operation_tuck(self, verify_height=True):
        """Park arms before lowering the leg (taught tuck; clear post-fold torso z)."""
        trajs = {}
        chain_joints = {
            "left_arm": list(self._LOW_OPERATION_TUCK_LEFT),
            "right_arm": list(self._LOW_OPERATION_TUCK_RIGHT),
        }
        for chain_name, joint_angles in chain_joints.items():
            trajs[chain_name] = self.plan_traj_from_joint_positions(joint_angles, chain_name)
        if any(t is None for t in trajs.values()):
            log.error("❌ low_operation_tuck: trajectory planning failed")
            return False
        if not self.execute_trajectories(trajs):
            log.error("❌ low_operation_tuck: execute failed")
            return False
        if verify_height and not self._verify_low_operation_tuck_height():
            log.error(
                f"❌ low_operation_tuck: elbow/wrist below LOW torso band "
                f"({self._LOW_OP_TUCK_MIN_Z_M:.3f} m)"
            )
            return False
        log.info("✅ low_operation_tuck OK")
        return True

    def _verify_low_operation_tuck_height(self):
        """Elbow+wrist must clear post-fold LOW torso height (not standing leg_link5)."""
        floor = self._LOW_OP_TUCK_MIN_Z_M - self._LOW_OP_TUCK_Z_EPS_M
        for side in ("left_arm", "right_arm"):
            for kind, frame in (
                ("elbow", self._LOW_OP_TUCK_ELBOW[side]),
                ("wrist", self._LOW_OP_TUCK_WRIST[side]),
            ):
                pose = self.get_relative_pose(frame, "base_link")
                if pose is None or len(pose) < 3:
                    log.warning(f"⚠️ low_operation_tuck: no TF for {frame}; skip height verify")
                    return True
                z = float(pose[2])
                log.info(
                    f"[low_operation_tuck] {side} {kind} z={z:.3f} "
                    f"(need ≥ {floor:.3f} = LOW torso band)"
                )
                if z < floor:
                    return False
        return True


    def get_relative_pose(self, target_frame, reference_frame='base_link'):
        try:
            pose_data = self.robot.get_transform(reference_frame, target_frame, timeout_ms=5000)
            # pose_data format: ([x, y, z, qx, qy, qz, qw], timestamp)
            return pose_data[0]
        except Exception as e:
            log.warning(f"Error getting transform: {e}")
            return None


    def get_arm_ee_pose(self, target_chain='left_arm', reference_frame='base_link'):
        try:
            status, original_pose = self.motion.get_end_effector_pose_on_chain(
                chain_name=target_chain,
                frame_id="EndEffector",
                reference_frame=reference_frame
            )
            assert status == MotionStatus.SUCCESS, "Failed to get end-effector pose"
            log.info(f"✅ Current {target_chain} end-effector pose: {original_pose}")
            time.sleep(0.8)
        except Exception as e:
            log.error(f"❌ Exception getting end-effector pose by chain name: {e}")
            return None
        return original_pose


    def check_traj_status(self, chain_names=["left_arm"]):
        status = self.robot.check_trajectory_execution_status(chain_names)
        return status


    def check_arm_ee_ik(self, ee_pose, target_chain='left_arm', reference_frame='base_link') -> bool:
        """Control the robotic arm end-effector to move to the specified position"""
        joint_angles_ik = {target_chain: None}
        try:
            status, joint_angles_ik = self.motion.inverse_kinematics(
                target_pose=ee_pose,
                chain_names=[target_chain],
                target_frame="EndEffector",
                reference_frame=reference_frame,
                enable_collision_check=True # Disable collision detection
            )
            assert status == MotionStatus.SUCCESS, f"IK solving failed, status: {status}"
            log.info(f"✅ Target {target_chain} IK solving successful joint_angles_ik: {joint_angles_ik}")
            return True
        except Exception as e:
            log.error(f"❌ IK solving exception: {e}")
            return False

    def set_arm_ee_pose_ik(self, ee_pose, target_chain='left_arm', reference_frame='base_link') -> None:
        joint_angles_ik = {target_chain: None}
        try:
            status, joint_angles_ik = self.motion.inverse_kinematics(
                target_pose=ee_pose,
                chain_names=[target_chain],
                target_frame="EndEffector",
                reference_frame=reference_frame,
                enable_collision_check=True # Disable collision detection
            )
            assert status == MotionStatus.SUCCESS, f"IK solving failed, status: {status}"
        except Exception as e:
            log.error(f"❌ IK solving exception: {e}")
            return

        if joint_angles_ik[target_chain] is not None:
            self.robot.set_joint_positions(
                joint_angles_ik[target_chain], [target_chain], [], True,
                self._joint_speed_for_chain(target_chain), self.timeout_s
            )
        return

    def set_arm_ee_pose_motion_plan(self, ee_pose, target_chain='left_arm', reference_frame='base_link', blocking=True) -> None:
        """Control the robotic arm end-effector to move to the specified position"""
        traj = self.plan_traj_from_ee_pose(ee_pose, target_chain, reference_frame)
        if traj is None:
            return False
        return self.execute_trajectories({target_chain: traj}, blocking)


    def plan_traj_from_ee_pose(
        self,
        ee_pose,
        target_chain='left_arm',
        reference_frame='base_link',
        start=None,
        reference_robot_states=None,
        enable_collision_check=True,
    ) -> List[List[float]]:
        """Plan trajectory with end-effector pose target, return joint trajectory points.

        Plan-only (does not execute). Optional ``start`` (JointStates/PoseState) and
        ``reference_robot_states`` (RobotStates) seed the planner — e.g. hypothetical
        leg/torso height — without moving the robot.
        """
        custom_param = Parameter()
        if target_chain == "leg":
            custom_param.set_move_line(True)
        custom_param.set_timeout(self.timeout_s)
        target_robot_state = PoseState()
        target_robot_state.chain_name = target_chain
        target_robot_state.frame_id = "EndEffector"
        target_robot_state.reference_frame = reference_frame
        target_robot_state.pose = Pose(ee_pose)

        for attempt in range(3): # try 3 times
            try:
                status, traj = self.motion.motion_plan(
                    target=target_robot_state,
                    start=start,
                    reference_robot_states=reference_robot_states,
                    enable_collision_check=enable_collision_check,
                    params=custom_param
                )
                assert status == MotionStatus.SUCCESS, f"Motion Planning failed for {target_chain} target ee pose: {ee_pose} status: {status}"
                if traj != {}:
                    if self.debug:
                        log.info(f"✅ joint chain {target_chain} motion planning succeed")
                    return traj[target_chain]
                else:
                    if self.debug:
                        log.warning(f"⚠️ return succeed but empty trajectory, probably arrived target joint state, skipping this chain: {target_chain}")
                    return []
            except Exception as e:
                log.error(f"❌ plan_traj_from_ee_pose failed exception: {e}, retrying... (attempt {attempt+1}/3)")
                continue
        return None


    def plan_traj_from_joint_positions(self, joint_positions, target_chain='left_arm') -> List[List[float]]:
        """Plan trajectory with joint position targets, return joint trajectory points"""
        custom_param = Parameter()
        if target_chain == "leg":
            custom_param.set_move_line(True)
        custom_param.set_timeout(self.timeout_s)
        target_joint = JointStates()
        target_joint.chain_name = target_chain
        target_joint.joint_positions = joint_positions

        for attempt in range(3): # try 3 times
            try:
                status, traj = self.motion.motion_plan(
                    target=target_joint,
                    enable_collision_check=True,
                    params=custom_param
                )
                assert status == MotionStatus.SUCCESS, f"Motion Planning failed for {target_chain} target joint_positions: {joint_positions} status: {status}"
                if traj != {}:
                    if self.debug:
                        log.info(f"✅ joint chain {target_chain} motion planning succeed")
                    return traj[target_chain]
                else:
                    if self.debug:
                        log.warning(f"⚠️ return succeed but empty trajectory, probably arrived target joint state, skipping this chain: {target_chain}")
                    return []
            except Exception as e:
                log.error(f"❌ plan_traj_from_joint_positions failed exception: {e}, retrying... (attempt {attempt+1}/3)")
                continue
        return None

    def execute_trajectories(self, trajs, blocking=True):
        # None = planning failure; [] = already at target (skip that chain).
        if any(trajs[chain] is None for chain in trajs.keys()):
            failed = [c for c, t in trajs.items() if t is None]
            log.error(f"❌ execute_trajectories: planning failed for chains {failed}")
            return False
        valid_joint_groups = [chain for chain in trajs.keys() if len(trajs[chain]) > 0]
        if not valid_joint_groups:
            log.info("No non-empty trajectories (already at targets); treating as success.")
            return True

        filtered_trajs = {chain: trajs[chain] for chain in valid_joint_groups}

        exec_traj = Trajectory()
        exec_traj.joint_groups = valid_joint_groups
        exec_traj.joint_names = []
        traj_points = []
        curr_time = 0.0

        max_len = max(len(t) for t in filtered_trajs.values())
        total_features = sum(len(t[0]) for t in filtered_trajs.values())
        joint_traj = np.zeros((max_len, total_features))

        chain_spans = []
        current_col = 0
        for chain in valid_joint_groups:
            t = np.array(filtered_trajs[chain])
            n_rows, n_cols = t.shape
            col_end = current_col + n_cols
            joint_traj[:n_rows, current_col:col_end] = t
            if n_rows < max_len:
                joint_traj[n_rows:, current_col:col_end] = t[-1, :]
            chain_spans.append((chain, current_col, col_end))
            current_col = col_end

        for i, joint_angles in enumerate(joint_traj):
            if i == 0:
                dt = 0.01
            else:
                dt = 0.01
                prev = joint_traj[i - 1]
                for chain, c0, c1 in chain_spans:
                    delta = float(np.max(np.abs(joint_angles[c0:c1] - prev[c0:c1])))
                    speed = self._joint_speed_for_chain(chain)
                    if speed > 0.0:
                        dt = max(dt, delta / speed)
            curr_time += dt
            traj_point = generate_target_point(joint_angles, curr_time)
            traj_points.append(traj_point)
        exec_traj.points = traj_points

        try:
            status = self.robot.execute_joint_trajectory(exec_traj, blocking)
            time.sleep(0.1) # small sleep to ensure fully excute
            assert status == ControlStatus.SUCCESS, f"Failed to execute trajectories with status: {status}"
            return True
        except Exception as e:
            log.error(f"❌ Failed to execute trajectories: {e}")
            return False

    def test_set_leg_ee(self) -> bool:
        try:
            custom_param = Parameter()
            # Add logic to switch to linear motion
            custom_param.set_move_line(True)
            ee_pose= [0.0, 0.0, 0.835220900883465, 0.5, 0.5, 0.5, 0.5]

            status = self.motion.set_end_effector_pose(
                target_pose=ee_pose,
                end_effector_frame="leg",
                reference_frame="base_link",
                enable_collision_check=True,
                is_blocking=True,
                timeout=-1.0,
                params=custom_param
            )
            assert status == MotionStatus.SUCCESS, f"set end effector pose failed pose: {ee_pose} target_chain: leg status: {status}"
        except Exception as e:
            log.error(f"❌ set arm ee pose failed exception: {e}")
            return False

    def set_arm_ee_pose(self, ee_pose, target_chain='left_arm', reference_frame='base_link', blocking=True) -> bool:

        try:
            custom_param = Parameter()
            # Add logic to switch to linear motion
            if target_chain == "leg":
                custom_param.set_move_line(True)
            status = self.motion.set_end_effector_pose(
                target_pose=ee_pose,
                end_effector_frame=target_chain,
                reference_frame=reference_frame,
                enable_collision_check=True,
                is_blocking=blocking,
                timeout=-1.0,
                params=custom_param
            )
            assert status == MotionStatus.SUCCESS, f"set end effector pose failed pose: {ee_pose} target_chain: {target_chain} status: {status}"
        except Exception as e:
            log.error(f"❌ set arm ee pose failed exception: {e}")
            return False

    def set_joint_angles(self, joint_angles, chain_name):
        try:
            self.robot.set_joint_positions(
                joint_angles, [chain_name], [], True,
                self._joint_speed_for_chain(chain_name), self.timeout_s
            )
            log.info(f"✅ Setting head joint angles successful: {joint_angles}")
        except Exception as e:
            log.error(f"❌ Setting head joint angles exception: {e}")

    def get_torso_pose(self, reference_frame='base_link'):
        """Get Torso height by calculating the end-effector pose of the leg chain"""
        try:
            status, original_pose = self.motion.get_end_effector_pose_on_chain(
                chain_name="leg",
                frame_id="EndEffector",
                reference_frame=reference_frame
            )
            assert status == MotionStatus.SUCCESS, "Failed to get end-effector pose"
            return original_pose
        except Exception as e:
            log.error(f"❌ Exception getting end-effector height: {e}")
            return None

    def _leg_ik_seed(self):
        """Current leg joints for IK seed (MPS often cannot read WBC without this)."""
        try:
            rs = self.motion.get_robot_states()
            wb = list(getattr(rs, "whole_body_joint", None) or [])
            if len(wb) >= 5:
                return [float(v) for v in wb[:5]]
        except Exception as e:
            log.warning(f"get_robot_states for leg IK seed failed: {e}")
        # Match set_init_pose scan leg if live state unavailable.
        return [0.32, 1.14, 0.82, 0.0, 0.0]

    def set_torso_pose(self, pose, reference_frame='base_link'):
        """Control the torso to move to the specified position by calculating the IK of the leg chain"""
        joint_angles_ik = {'leg': None}
        custom_param = Parameter()
        custom_param.set_move_line(True)
        seed = self._leg_ik_seed()

        try:
            status, joint_angles_ik = self.motion.inverse_kinematics(
                target_pose=pose,
                chain_names=['leg'],
                target_frame="EndEffector",
                reference_frame=reference_frame,
                initial_joint_positions={"leg": list(seed)},
                enable_collision_check=True, # Disable collision detection
                params=custom_param
            )
            assert status == MotionStatus.SUCCESS, "IK solving failed"
            log.info(f"✅ Target leg IK solving successful joint_angles_ik: {joint_angles_ik}")
        except Exception as e:
            log.error(f"❌ IK solving exception: {e}")
            return

        if joint_angles_ik['leg'] is not None:
            self.robot.set_joint_positions(
                joint_angles_ik['leg'], ['leg'], [], True,
                self._joint_speed_for_chain("leg"), self.timeout_s
            )
        return

    def set_torso_height_upright(
        self,
        z,
        reference_frame='base_link',
        x_min=-0.05,
        x_max=0.25,
        x_step=0.01,
    ):
        """Move torso EE to height `z` with upright orientation; free-x IK.

        Locks z and upright quat (from live EE); holds live y; searches x with
        warm seed continuity (same approach as probe_g1_leg_height). Does not
        hardcode x — folding the leg may change x; base compensation is separate.

        Returns True on success.
        """
        z = float(z)
        live = self.get_torso_pose(reference_frame=reference_frame)
        if live is None or len(live) < 7:
            log.error("❌ set_torso_height_upright: no live torso EE pose")
            return False

        y = float(live[1])
        quat = list(live[3:7])
        live_x = float(live[0])
        seed = self._leg_ik_seed()
        custom_param = Parameter()
        custom_param.set_move_line(True)
        try:
            custom_param.set_timeout(5.0)
        except Exception:
            pass

        def _ik_at_x(xv, seed_leg):
            pose = [float(xv), y, z, *quat]
            status, joint_map = self.motion.inverse_kinematics(
                target_pose=pose,
                chain_names=["leg"],
                target_frame="EndEffector",
                reference_frame=reference_frame,
                initial_joint_positions={"leg": list(seed_leg)},
                enable_collision_check=True,
                params=custom_param,
            )
            if status != MotionStatus.SUCCESS or not joint_map or not joint_map.get("leg"):
                return None
            return list(joint_map["leg"])

        # Stage 1: keep live x
        best = None  # (cost, joints, x)
        j = _ik_at_x(live_x, seed)
        if j is not None:
            best = (0.0, j, live_x)
        else:
            # Stage 2: neighborhood around live x
            for d in (0.01, -0.01, 0.02, -0.02, 0.03, -0.03, 0.04, -0.04, 0.05, -0.05):
                xv = live_x + d
                if xv < float(x_min) - 1e-9 or xv > float(x_max) + 1e-9:
                    continue
                j = _ik_at_x(xv, seed)
                if j is None:
                    continue
                cost = float(np.linalg.norm(np.asarray(j[:5]) - np.asarray(seed[:5])))
                if best is None or cost < best[0]:
                    best = (cost, j, xv)
            # Stage 3: coarse grid
            if best is None:
                xv = float(x_min)
                while xv <= float(x_max) + 1e-9:
                    j = _ik_at_x(xv, seed)
                    if j is not None:
                        cost = float(
                            np.linalg.norm(np.asarray(j[:5]) - np.asarray(seed[:5]))
                        )
                        if best is None or cost < best[0]:
                            best = (cost, j, float(xv))
                    xv += float(x_step)

        if best is None:
            log.error(f"❌ set_torso_height_upright: no free-x IK at z={z:.3f}")
            return False

        _, joints, xv = best
        log.info(
            f"✅ torso height upright z={z:.3f} free-x={xv:.3f} "
            f"leg={np.round(joints, 4).tolist()}"
        )
        status = self.robot.set_joint_positions(
            list(joints), ["leg"], [], True, self._joint_speed_for_chain("leg"), self.timeout_s
        )
        if status != ControlStatus.SUCCESS:
            log.error(f"❌ set_torso_height_upright set_joint_positions failed: {status}")
            return False
        return True

    # ----------------------------------------------------------------- base
    #
    # TWO PREDECESSORS DISAGREED ABOUT WHAT set_base_velocity MEANT, and
    # collapsing them silently would have changed robot behaviour:
    #
    #   the task path   -> navigation.navigate_with_velocity(...,
    #                      enable_collision_check=True). Collision checked. Tasks
    #                      issue duration_s-bounded pulses and depend on that
    #                      checking.
    #   the teleop path -> raw robot.set_base_velocity(). NO collision check.
    #                      Teleop deliberately reverted to this because the nav
    #                      stack's blocking status monitor made the joystick
    #                      jerky.
    #
    # Both paths are kept, under names that say which is which. Teleop uses
    # the raw one by default (TELEOP_RAW_VELOCITY=1); tasks use the checked one.

    def set_base_velocity(self, x_vel=0, y_vel=0, yaw_vel=0, duration_s=0.1,
                          enable_collision_check=True):
        """Drive the base through the navigation stack. Collision-checked.

        Non-blocking: returns as soon as the command is accepted. `duration_s`
        is the SDK-side auto-stop for this command.

        FIXED: the original fell off the end of the function after logging
        success, returning None -- so every successful call reported failure to
        its caller (and the poc's teleop surfaced {"success": false} while the
        robot was actually moving).
        """
        try:
            success, status = self.navigation.navigate_with_velocity(
                x_vel, y_vel, yaw_vel,
                duration_s=duration_s,
                enable_collision_check=enable_collision_check)
            if not success:
                log.error(f"❌ Set Base Velocity to ({x_vel}, {y_vel}, {yaw_vel}) "
                         f"failed with status: {status}")
                return False
            return True
        except Exception as e:
            log.error(f"❌ Exception moving base: {e}")
            return False

    def move_straight(self, dx, dy=0.0, timeout_s=8.0):
        """Move the base (dx, dy) metres relative to its current base_link pose.

        Wraps `GalbotNavigation.move_straight_to`: a straight odom-frame move
        with no global path planning, blocking until done or `timeout_s`.
        Heading is kept (identity rotation). Needs CHASSIS_POSE_CTRL, which the
        constructor sets and nothing switches away from.
        """
        goal = np.array([float(dx), float(dy), 0.0, 0.0, 0.0, 0.0, 1.0], dtype=np.float64)
        try:
            success, status = self.navigation.move_straight_to(
                goal, is_blocking=True, timeout=float(timeout_s))
        except Exception as e:
            log.error(f"❌ Exception in move_straight_to({dx:.3f}, {dy:.3f}): {e}")
            return False
        if not success:
            log.error(f"❌ move_straight_to({dx:.3f}, {dy:.3f}) failed with status: {status}")
            return False
        return True

    def set_base_velocity_raw(self, x_vel=0, y_vel=0, yaw_vel=0, duration_s=0.0):
        """Drive the base directly, bypassing the navigation stack.

        NO COLLISION CHECKING. This is the responsive path teleop uses.

        `duration_s=0.0` means "no SDK-side auto-stop" -- the caller owns the
        deadman. utils/teleop.py's watchdog is that deadman. Passing a non-zero
        duration here schedules an independent stop timer per call, which fires
        even after newer commands have arrived; that was the root cause of the
        teleop jerkiness that was chased down at the time, so leave it at 0.
        """
        try:
            status = self.robot.set_base_velocity(
                linear_velocity=[x_vel, y_vel, 0],   # [vx, vy, vz] m/s
                angular_velocity=[0, 0, yaw_vel],    # [wx, wy, wz] rad/s
                duration_s=duration_s)
            if status == ControlStatus.SUCCESS:
                return True
            log.error(f"❌ Raw base velocity ({x_vel}, {y_vel}, {yaw_vel}) "
                     f"failed with status: {status}")
            return False
        except Exception as e:
            log.error(f"❌ Exception moving base (raw): {e}")
            return False


    # ---------------- monitored (blocking) velocity -------------------
    # Terminal statuses for a velocity command's monitoring loop -- everything
    # `NavigationTaskStatus` can report except the two "still going" values
    # (RUNNING, UNKNOWN). Matches GalbotSDK's own
    # `examples/g1/python/galbot_navigation/navigate_with_velocity.py`
    # (`monitor_navigation`'s break condition).
    _VELOCITY_TERMINAL_STATUSES = {
        NavigationTaskStatus.SUCCESS,
        NavigationTaskStatus.FAILED,
        NavigationTaskStatus.INTERRUPTED,
        NavigationTaskStatus.OCCUPIED,
        NavigationTaskStatus.COLLISION,
        NavigationTaskStatus.CLOSE_TO_OBSTACLE,
    }

    def set_base_velocity_monitored(self, cmd_vel, duration_s=0.5,
                                    enable_collision_check=True,
                                    poll_interval_s=0.2, timeout_margin_s=2.0):
        """BLOCKING velocity drive that reports why it stopped.

        Written and then deliberately unwired from teleop (the blocking monitor
        made the joystick jerky). Kept because it is the ONLY path that surfaces
        COLLISION / CLOSE_TO_OBSTACLE / OCCUPIED to the caller, which a task
        that needs to know whether a nudge actually happened can use.

        Currently no caller. Do not wire this to teleop.

        Original docstring follows.

        Drive the base at a velocity command through the navigation stack
        (`GalbotNavigation.navigate_with_velocity`) instead of the raw
        `robot.set_base_velocity` used by `set_base_velocity` above -- this
        goes through nav2's costmap, so `enable_collision_check=True` (the
        default, kept mandatory-looking here on purpose) actually stops the
        base before a collision instead of just commanding the wheels.

        Blocking: the underlying SDK call itself only returns once the
        command is *accepted*, not once it's done (per its own docstring).
        This method additionally polls `navigation.get_navigation_status()`
        every `poll_interval_s` -- mirroring GalbotSDK's own
        `navigate_with_velocity.py` example's `monitor_navigation` helper --
        until a terminal status is reported or `duration_s + timeout_margin_s`
        elapses, so callers (e.g. teleop) know the command actually finished
        (or was cut short by a collision/obstacle stop) before moving on.

        Parameters:
            cmd_vel: [x_vel, y_vel, yaw_vel] -- linear x/y (m/s) + yaw (rad/s).
            duration_s: command duration in seconds (must be > 0).
            enable_collision_check: must stay True for teleop use.
            poll_interval_s: how often to poll navigation status while blocking.
            timeout_margin_s: extra time allowed past `duration_s` before
                giving up on monitoring -- a velocity command has no
                destination, so it commonly never reports SUCCESS and just
                runs out its duration; that's treated as a normal finish, not
                a failure (again, matching the example).

        Returns:
            bool: False if the command was rejected outright, or if
                monitoring observed a FAILED/INTERRUPTED/OCCUPIED/COLLISION/
                CLOSE_TO_OBSTACLE status (the base is stopped explicitly in
                that case). True otherwise.
        """
        x_vel, y_vel, yaw_vel = cmd_vel
        try:
            accepted, msg = self.navigation.navigate_with_velocity(
                vx=x_vel, vy=y_vel, vyaw=yaw_vel,
                duration_s=duration_s, enable_collision_check=enable_collision_check)
        except Exception as e:
            log.error(f"❌ Exception in navigate_with_velocity: {e}")
            return False

        if not accepted:
            log.error(f"❌ navigate_with_velocity ({x_vel}, {y_vel}, {yaw_vel}) rejected: {msg}")
            return False

        log.info(f"✅ navigate_with_velocity ({x_vel}, {y_vel}, {yaw_vel}) accepted: {msg}, monitoring...")

        timeout_s = duration_s + timeout_margin_s
        start_time = time.time()
        final_status = None
        while time.time() - start_time < timeout_s:
            try:
                status = self.navigation.get_navigation_status()
            except Exception as e:
                log.error(f"❌ Exception polling navigation status: {e}")
                break
            if status in self._VELOCITY_TERMINAL_STATUSES:
                final_status = status
                break
            time.sleep(poll_interval_s)

        if final_status is None or final_status == NavigationTaskStatus.SUCCESS:
            label = nav_status_str.get(final_status, "duration elapsed")
            log.info(f"✅ navigate_with_velocity ({x_vel}, {y_vel}, {yaw_vel}) finished: {label}")
            return True

        log.error(f"❌ navigate_with_velocity ({x_vel}, {y_vel}, {yaw_vel}) stopped early: "
                  f"{nav_status_str.get(final_status, final_status)}")
        self.navigation.stop_navigation()
        return False

    def _joint_speed_for_chain(self, chain):
        if chain == "leg":
            return float(getattr(self, "leg_speed_rad_s", self.max_speed_rad_s) or self.max_speed_rad_s)
        if chain in ("left_arm", "right_arm"):
            return float(getattr(self, "arm_speed_rad_s", self.max_speed_rad_s) or self.max_speed_rad_s)
        return float(self.max_speed_rad_s)

    def get_robot_speeds(self):
        return {
            "base_linear_mps": float(self.base_linear_mps),
            "base_angular_rad_s": float(self.base_angular_rad_s),
            "leg_speed_rad_s": float(self.leg_speed_rad_s),
            "arm_speed_rad_s": float(self.arm_speed_rad_s),
        }

    def apply_robot_speeds(self, params):
        self.base_linear_mps = float(params["base_linear_mps"])
        self.base_angular_rad_s = float(params["base_angular_rad_s"])
        self.leg_speed_rad_s = float(params["leg_speed_rad_s"])
        self.arm_speed_rad_s = float(params["arm_speed_rad_s"])
        return self.apply_nav_speed_limits()

    def apply_nav_speed_limits(self):
        linear = float(self.base_linear_mps)
        angular = float(self.base_angular_rad_s)
        vel_limit = np.array([linear, linear, angular], dtype=np.float64)
        ret, status = self.navigation.set_navigation_velocity_limit(vel_limit)
        if ret:
            self.nav_speed_mode = "configured"
            self.nav_linear_limit_mps = linear
            self.nav_angular_limit_rad_s = angular
            log.info(
                f"nav velocity limit vx=vy={linear:.3f} m/s vyaw={angular:.3f} rad/s"
            )
        else:
            log.warning(f"Failed to set navigation velocity limit, status: {status}")
        return ret, status

    # set_slow_navigation / set_fast_navigation used to be distinct, then both
    # collapsed to apply_nav_speed_limits() while callers kept using the old
    # names as if the distinction still existed. Callers now call
    # apply_nav_speed_limits() directly; the names are gone rather than
    # preserved, so nobody writes new code expecting a slow/fast difference.

    def get_nav_speed_limits(self):
        """Current (linear_mps, angular_rad_s) navigation velocity limit."""
        return self.nav_linear_limit_mps, self.nav_angular_limit_rad_s

    def get_nav_pose(self):
        try:
            return self.navigation.get_current_pose()
        except Exception as e:
            log.warning(f"Error getting navigation pose: {e}")
            return None

    def get_battery_level(self):
        """Return BMS battery_level (0–100) or None if unavailable."""
        try:
            info = self.robot.get_bms_information()
            if not info:
                return None
            level = info.get("battery_level")
            if level is None:
                return None
            return float(level)
        except Exception as e:
            log.warning(f"Error getting battery level: {e}")
            return None

    def get_head_joint_pose(self):
        """get head joint angles"""
        try:
            head_joint_angles = self.robot.get_joint_positions(["head"], [])
            # assert status == MotionStatus.SUCCESS, "Failed to get head joint angles"
            return head_joint_angles
        except Exception as e:
            log.error(f"❌ Exception getting head joint angles: {e}")
            return None

    def move_head_joints(self, joint_angles):
        """move head joints to specified angles"""
        try:
            self.robot.set_joint_positions(
                joint_angles, ['head'], [], True, self.max_speed_rad_s, self.timeout_s
            )
            log.info(f"✅ Setting head joint angles successful: {joint_angles}")
        except Exception as e:
            log.error(f"❌ Setting head joint angles exception: {e}")

    def move_head_joints_live(self, joint_angles, speed_rad_s):
        """Replace a head target without blocking the teleop/deadman loop."""
        return self.robot.set_joint_positions(
            joint_angles, ['head'], [], False, min(0.5, max(0.01, speed_rad_s)), self.timeout_s)

    def get_left_arm_image(self):
        """get compressed image data"""
        rgb_image_data = self.robot.get_rgb_data(SensorType.LEFT_ARM_CAMERA)
        return decode_compressed_image(rgb_image_data)

    def get_left_arm_depth(self):
        """get depth image data"""
        depth_image_data = self.robot.get_depth_data(SensorType.LEFT_ARM_DEPTH_CAMERA)
        return decode_compressed_image(depth_image_data)

    def get_arm_image(self, side="left"):
        """get compressed image data"""
        if side=="left":
            sensor_type = SensorType.LEFT_ARM_CAMERA
        else:
            sensor_type = SensorType.RIGHT_ARM_CAMERA
        rgb_image_data = self.robot.get_rgb_data(sensor_type)
        if not rgb_image_data :
            return None
        return decode_compressed_image(rgb_image_data)

    def get_arm_depth(self, side="left"):
        """get depth image data"""
        if side=="left":
            sensor_type = SensorType.LEFT_ARM_DEPTH_CAMERA
        else:
            sensor_type = SensorType.RIGHT_ARM_DEPTH_CAMERA
        depth_image_data = self.robot.get_depth_data(sensor_type)
        if not depth_image_data:
            return None
        return decode_compressed_image(depth_image_data)

    def get_head_image(self):
        """Head-left camera frame as BGR, or None.

        FIXED: the None guard that get_arm_image already had was missing here,
        so a missing frame reached decode_compressed_image(None) and raised
        TypeError. The enrollment QR-scan loop and the MJPEG stream both poll
        this before the camera is warm.
        """
        rgb_image_data = self.robot.get_rgb_data(SensorType.HEAD_LEFT_CAMERA)
        if not rgb_image_data:
            return None
        return decode_compressed_image(rgb_image_data)

    # ------------------------------------------------------------------
    # Compatibility shims for the infra modules.
    #
    # enrollment.py and calibration.py were
    # written against a much thinner robot client. These give them the same
    # names on this (far larger) GalbotControl, so those modules work unchanged.
    # ------------------------------------------------------------------

    HEAD_LEFT_FRAME = galbot_sensor_link_names["head_left"]
    ARM_FRAMES = {
        "left": galbot_sensor_link_names["left_arm"],
        "right": galbot_sensor_link_names["right_arm"],
    }

    @staticmethod
    def _sensor_key(name):
        """Normalize 'left' / 'left_arm' / a SensorType to a galbot_sensor_types key.

        The two source repos used two conventions -- the poc said "left"/"right"
        for arms, shelf_scanning said "left_arm"/"right_arm". Normalizing in one
        place stops that leaking into every call site.
        """
        if name in galbot_sensor_types:
            return name
        if name in ("left", "right"):
            return f"{name}_arm"
        for key, sensor_type in galbot_sensor_types.items():
            if sensor_type == name:
                return key
        raise ValueError(f"unknown camera {name!r}; valid: {sorted(galbot_sensor_types)}")

    def get_head_left_image(self):
        """poc alias for get_head_image()."""
        return self.get_head_image()

    def get_head_left_intrinsics(self):
        """Head-left camera K as a fresh 3x3."""
        return self.get_cam_intrinsics("head_left")

    def get_arm_intrinsics(self, side="left"):
        """Wrist camera K as a fresh 3x3. Accepts 'left'/'left_arm'.

        Returns a NEW array every call -- see get_arm_camera_intrinsics_matrix
        for why caching this would silently corrupt calibration.
        """
        return self.get_cam_intrinsics(self._sensor_key(side))

    def get_joint_names(self):
        """Active joint names for joint_encoders (empty list if none reported)."""
        names = list(self.robot.get_joint_names(True, []) or [])
        if not names:
            names = list(self.robot.get_joint_names(False, []) or [])
        return names

    def get_joint_positions(self, names):
        """Absolute joint positions (rad) for `names`, or None on mismatch/error.

        Returning None on a length mismatch is deliberate: a consumer maps
        these onto a URDF-declared order, and a partial list would silently
        misalign the published encoder values.
        """
        if not names:
            return None
        try:
            states = self.robot.get_joint_states([], names)
        except Exception as e:
            log.warning(f"Error getting joint states: {e}")
            return None
        if states is None or len(states) != len(names):
            return None
        positions = []
        for state in states:
            if isinstance(state, dict):
                positions.append(float(state["position"]))
            else:
                positions.append(float(state.position))
        return positions

    def get_rgb_image(self, cam_type="left_arm"):
        sensor_type = galbot_sensor_types.get(cam_type, None)
        if sensor_type == None:
            return None
        rgb_image_data = self.robot.get_rgb_data(sensor_type)
        return decode_compressed_image(rgb_image_data)

    def get_depth_image(self, cam_type="left_arm"):
        sensor_type = galbot_sensor_types.get(cam_type, None)
        if sensor_type == None:
            return None
        elif sensor_type == SensorType.HEAD_LEFT_CAMERA:
            if not self.perception.run_once(PerceptionModule.FOUNDATION_STEREO):
                log.warning("failed to fetch head depth")
                return None
            if not self.perception.wait_for_new_result(
                PerceptionModule.FOUNDATION_STEREO, timeout_s=6.0):
                log.warning("Timed out waiting for inference result")
                return None
            ok, result = self.perception.get_latest_result(PerceptionModule.FOUNDATION_STEREO)
            if not ok:
                log.warning("get_latest_result failed")
                return None
            depth_map = result.instance_mask
            if depth_map is None:
                log.info("No depth map (instance_mask is empty)")
                return None
            depth_f = depth_map.astype(np.float32)
            return depth_f / 1000.0
        else:
            depth_image_data = self.robot.get_depth_data(sensor_type)
            return decode_compressed_image(depth_image_data)

    def get_cam_intrinsics(self, cam_type="left_arm"):
        sensor_type = galbot_sensor_types.get(cam_type, None)
        if sensor_type == None:
            return None
        intrinsics = self.robot.get_camera_intrinsic(sensor_type)
        return  np.array(intrinsics["K"]).reshape(3, 3)

    def get_cam_extrinsics(self, cam_type="left_arm"):
        sensor_type = galbot_sensor_types.get(cam_type, None)
        if sensor_type == None:
            return None
        res = self.robot.get_sensor_extrinsic(sensor_type)
        return  res[0]


    def get_synced_sensors(self, sensor_str=[]):
        """Return synced RGB images for the requested sensor keys.

        Args:
            sensor_str (list[str]): list of camera names from galbot_sensor_types.

        Returns:
            dict[str, np.array]: decoded OpenCV BGR image per requested key.
        """
        sensors = []
        key_map = {}
        for key in sensor_str:
            sensor_type = galbot_sensor_types.get(key)
            if sensor_type is None:
                log.info(f"Unknown sensor key: {key}")
                continue
            sensors.append(sensor_type)
            key_map[sensor_type] = key

        if not sensors:
            return {}

        obs = self.robot.get_synced_observation(sensors, True)

        # Remark: have to use this due to we starting with empty map
        # can switch back to get_transform if we using galbot map
        base_wrt_galbot = self.get_nav_pose()
        if not obs:
            log.warning("get_synced_observation failed")
            return {}

        rgb_map = getattr(obs, "rgb_data_map", None)
        if rgb_map is None:
            log.info("No rgb_data_map found in synced observation")
            return {}

        synced_images = {}
        obs_timestamp = None
        for sensor_type, key in key_map.items():
            frame: RgbData = rgb_map.get(sensor_type)
            if frame is None:
                log.info(f"No synced frame for sensor: {key}")
                continue
            try:
                sensor_data = {}
                sensor_data["image"] = decode_compressed_RgbData(frame)

                # obs sync based on the first sensor
                if obs_timestamp is None:
                    obs_timestamp = frame.header.timestamp_ns
                sensor_data["timestamp"] = obs_timestamp


                # Pose looked up AT THE FRAME'S OWN TIMESTAMP, not at "now" --
                # that is the whole point of this method, and why the
                # calibration loop uses it instead of grabbing an image and
                # then separately querying the current camera pose.
                #
                # FIXED: this call was made twice in a row with identical
                # arguments and the first result discarded -- a wasted TF
                # lookup (with a timeout) per camera per tick.
                sensor_data["pose_wrt_base"] = self.robot.get_transform(
                    "base_link",
                    galbot_sensor_link_names.get(key),
                    frame.header.timestamp_ns,
                )[0]
                sensor_data["base_wrt_galbot"] = base_wrt_galbot
                # A fresh 3x3 per call. See get_arm_camera_intrinsics_matrix:
                # DomainCalibrator scales a supplied matrix in place, so this
                # must never become a cached shared array.
                sensor_data["intrinsics"] = self.get_cam_intrinsics(key)
                synced_images[key] = sensor_data
            except Exception as e:
                log.warning(f"Failed to decode synced RGB for {key}: {e}")

        return synced_images

    def clear_obstacle(self):
        status = self.motion.clear_obstacle()
        if status == MotionStatus.SUCCESS:
            log.info("✅ Clear obstacle successful")
            return True
        else:
            log.error(f"❌ Clear obstacle failed with status: {status}")
            return False

    def add_box_obstacle(self, obstacle_id, obstacle_size, obstacle_pose, target_frame='world'):
        """Add obstacle to the environment for collision avoidance"""
        try:
            assert len(obstacle_pose) == 7, f"invalid object pose, length should be 7 not {len(obstacle_pose)}"
            status = self.motion.add_obstacle(
                obstacle_id=obstacle_id,
                obstacle_type="box",
                pose=obstacle_pose,
                scale=obstacle_size,
                target_frame=target_frame
            )
            if status == MotionStatus.SUCCESS:
                log.info(f"✅ Add obstacle {obstacle_id} successful")
                return True
            else:
                log.error(f"❌ Add obstacle {obstacle_id} failed with status: {status}")
                return False
        except Exception as e:
            log.error(f"❌ Exception adding obstacle: {e}")
            return False

    def navigate_along_traj(self, waypoints_arr):
        waypoints_poses = []
        for wp_arr in waypoints_arr:
            waypoints_poses.append(
                Pose(
                wp_arr)
            )
        handle = self.navigation.navigate_along_trajectory(waypoints_poses)
        if not handle.request_sent:
            log.error(f"❌ Failed to send navigate along traj task, msg: {handle.msg}")
            return False, None
        return  True, handle.task_id

    def monitor_nav_task_status(self, task_id, timeout=60, goal_pose=None):
        """Poll a navigation task until SUCCESS, a failure status, or `timeout`.

        With `goal_pose` (the final waypoint, nav frame), the base sitting within
        nav_tuning's arrival tolerance of it on two polls in a row also counts as
        success: after arrival the task's status has been seen to stay RUNNING or
        UNKNOWN instead of turning SUCCESS, which kept this loop waiting out the
        whole timeout and then failing a navigation that had in fact finished.
        """
        tuning = nav_tuning.get()
        arrived_polls = 0
        start_time = time.time()
        # NOTE: an `unknown_count` counter used to be initialised here and never
        # touched again -- evidently intended to bail out after N consecutive
        # UNKNOWN statuses. It never did, so a task stuck in UNKNOWN still spins
        # until `timeout`. Removed the dead variable; the missing bail-out is a
        # real (pre-existing) gap if you see nav tasks hanging the full timeout.
        while (time.time() - start_time) < timeout:
            snapshot = self.navigation.get_navigation_target_status(task_id)
            log.info(f"status: {nav_status_str.get(snapshot.status)} msg: {snapshot.msg!r}")
            if snapshot.status == NavigationTaskStatus.SUCCESS:
                return True
            elif snapshot.status in [NavigationTaskStatus.RUNNING, NavigationTaskStatus.UNKNOWN]:
                if pose_reached(self.get_nav_pose(), goal_pose,
                                tuning.arrival_xy_m, tuning.arrival_yaw_rad):
                    arrived_polls += 1
                    if arrived_polls >= 2:
                        log.info(
                            f"navigation task {task_id}: base within "
                            f"{tuning.arrival_xy_m:.2f} m / {tuning.arrival_yaw_rad:.2f} rad "
                            f"of the goal while status is "
                            f"{nav_status_str.get(snapshot.status)}; treating as arrived")
                        self.navigation.stop_navigation()
                        return True
                else:
                    arrived_polls = 0
                time.sleep(1.0)
                continue
            else:
                log.warning(f"navigation task failed with status: {nav_status_str.get(snapshot.status)}")
                self.navigation.stop_navigation()
                return False
        log.warning(f"navigation task timeout, task_id: {task_id}")
        self.navigation.stop_navigation()
        return False

    def shutdown(self):
        """Stop chassis nav first, then cleanup SDK resources.

        Every step is guarded independently: an exception in request_shutdown()
        used to skip both wait_for_shutdown() and destroy(), leaking the SDK
        session. Note that wait_for_shutdown() takes no timeout and can block
        indefinitely -- main()'s SHUTDOWN_GRACE_S timer is what bounds it, so
        this method must never be the only thing between a signal and exit.
        """
        for label, call in (
            ("stop_navigation",
             lambda: (self.navigation.stop_navigation()
                      if getattr(self, "navigation", None) is not None else None)),
            ("request_shutdown", lambda: self.robot.request_shutdown()),
            # wait_for_shutdown() is DELIBERATELY ABSENT.
            #
            # Measured on the robot with gdb: it is
            # galbot::embosa::WaitForShutdown(), polling in a clock_nanosleep
            # loop inside a pybind11 call that never releases the GIL. In this
            # app it never returns, so it starves every Python thread -- which is
            # why a 4s worker.join() overran past 12s and the signal guard could
            # not fire either. Ctrl-C looked completely dead.
            #
            # And because it never returned, destroy() below NEVER RAN. Keeping
            # the call achieved strictly less teardown than dropping it.
            #
            # It is not simply slow: in a standalone script it returns in 0.70s
            # every time, with perception on, with sensors polled, with a
            # concurrent poller thread, and constructed from either the main or a
            # worker thread. Something about the running app leaves embosa's
            # precondition unmet -- that part is inside a closed-source library.
            # request_shutdown() and destroy() are what actually release
            # resources and both return promptly, so those are what we call.
            ("destroy", lambda: self.robot.destroy()),
        ):
            try:
                call()
            except Exception as e:  # noqa: BLE001
                log.info(f"shutdown: {label} ignored: {e}")
