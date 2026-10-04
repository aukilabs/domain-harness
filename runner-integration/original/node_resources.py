"""What this robot offers over its peer: where it is, and how it is posed.

Restores what the libp2p cluster used to publish, on the SDK's own peer. The
resource shapes and the frame conventions are recovered from that code rather
than re-derived, because one of them was learned the hard way.

FRAME CONVENTIONS ARE NOT A DETAIL HERE. The wire frames `base_link` and
`domain` are BOTH advertised as OpenGL, because `DomainCalibrator` and the task
code treat the domain as OpenGL (floor = x/z, height = y) and this publishes
that matrix as-is. Labelling `base_link` as ros_body once made a consumer morph
ros->opengl and shove depth into height, putting the robot about eight metres
below the floor. Nothing downstream can detect that: the manifest resolves and
the hash matches.

The URDF is a separate matter and stays `ros_body`, which is its own internal
convention. So this robot legitimately mixes the two -- exactly the case the
protocol refuses to give a default for.

The recording path's `auki_recording.frame_registry` declares ros_body/ros_optical
for its sensor frames. That is a different context: those frames describe the
robot's own geometry, not a pose expressed in domain coordinates.
"""

from __future__ import annotations

import json
import os
import re
import time

import auki_runner_protocol as p
from auki_robot_node import LiveResource

import config
import display_meshes
import urdf_manifest
from utils.logs import get_logger
from utils.transforms import convert_pose_to_transform, convert_transform_to_pose

log = get_logger(__name__)

BASE_FRAME_ID = "base_link"
DOMAIN_FRAME_ID = "domain"
POSE_RESOURCE_ID = "{}->{}".format(BASE_FRAME_ID, DOMAIN_FRAME_ID)
JOINTS_RESOURCE_ID = "joint_encoders"
HEAD_LEFT_CAMERA = "head_left"
CAMERA_RESOURCE_ID = "camera/head_left"

#: See the module docstring. Both are OpenGL, and they must agree: the two
#: resources share `base_link`, and the protocol refuses a frame id that two
#: resources describe differently. The head camera uses that same `base_link`:
#: `ros_optical` is not a legal convention, and Peyote displays the JPEG
#: without placing it from this frame.
WIRE_CONVENTION = "opengl"

DEFAULT_POSE_FPS = 2
DEFAULT_JOINT_FPS = 10
DEFAULT_CAMERA_FPS = 12
DEFAULT_CAMERA_JPEG_QUALITY = 70
#: Long-edge cap for head_left. JPEG quality does not change the bitmap
#: Chrome allocates; a full-sensor frame is what traps the renderer.
MAX_CAMERA_EDGE = 640


def _fps(name, default):
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        value = int(raw)
    except ValueError:
        log.warning("%s=%r is not an integer; using %d", name, raw, default)
        return default
    return value if value > 0 else default


def _jpeg_quality(name, default):
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        value = int(raw)
    except ValueError:
        log.warning("%s=%r is not an integer; using %d", name, raw, default)
        return default
    if not 1 <= value <= 100:
        log.warning("%s=%r is outside 1..100; using %d", name, raw, default)
        return default
    return value


def robot_urdf_path():
    """The URDF, which is both the device model and the joint wire order."""
    raw = os.environ.get("AUKI_ROBOT_URDF", "").strip()
    if not raw:
        return None
    return raw if os.path.isfile(raw) else None


def urdf_active_joint_names(urdf_path):
    """Revolute/continuous joint names in URDF declaration order.

    This is the wire order, and it is the consumer's only way to map an index
    back to a link: joint NAMES never go on the wire, only a bare float array.
    """
    with open(urdf_path, "r", errors="replace") as handle:
        text = handle.read()
    names = []
    for match in re.finditer(r"<joint\b([^>]*)>", text, flags=re.IGNORECASE):
        attrs = match.group(1)
        name_m = re.search(r"""\bname=["']([^"']+)["']""", attrs, flags=re.IGNORECASE)
        type_m = re.search(r"""\btype=["']([^"']+)["']""", attrs, flags=re.IGNORECASE)
        if not name_m or not type_m:
            continue
        if type_m.group(1).lower() in ("revolute", "continuous"):
            names.append(name_m.group(1))
    return names


def angles_in_urdf_order(urdf_names, sdk_names, sdk_positions):
    """Map SDK samples onto URDF order; missing joints -> 0.0, extras dropped."""
    by_name = {}
    for name, position in zip(sdk_names, sdk_positions):
        by_name[name] = float(position)
    return [by_name.get(name, 0.0) for name in urdf_names]


# ---------------------------------------------------------------- the sources

def make_pose_source(ctx, rate_hz):
    """base_link in domain coordinates, composed fresh on every tick.

    Calibration stores `T_domain_galbot` (domain <- odom origin), which only
    changes when a portal is sighted; the live half is the nav pose. Composing
    them here is what makes the wire sample an actual current pose rather than
    wherever the robot was standing when it last saw a portal.
    """
    period = 1.0 / max(rate_hz, 1)

    def sample():
        calibrated, _domain_id, t_domain_galbot, _updated = \
            ctx.calibration_state.snapshot()
        if not calibrated or t_domain_galbot is None:
            return None
        base_wrt_galbot = ctx.galbot.get_nav_pose()
        if base_wrt_galbot is None:
            return None
        t_domain_base = t_domain_galbot @ convert_pose_to_transform(base_wrt_galbot)
        return list(convert_transform_to_pose(t_domain_base))

    def source():
        import asyncio

        from auki_datatypes.auki.pose import Quat, SpatialTransform, Vec3

        async def pull():
            while True:
                values = sample()
                if values is not None:
                    x, y, z, qx, qy, qz, qw = values
                    message = SpatialTransform(
                        translation=Vec3(x=x, y=y, z=z),
                        orientation=Quat(x=qx, y=qy, z=qz, w=qw))
                    yield {"timestamp_ns": time.time_ns(),
                           "payload": bytes(message)}
                # A GAP IS A SKIPPED FRAME. An uncalibrated tick yields nothing
                # and keeps going; returning would end the subscription.
                await asyncio.sleep(period)

        return pull()

    return source


def make_joint_source(ctx, rate_hz, urdf_names, sdk_names):
    """Joint angles in URDF order, as a bare float array.

    The SDK's joint set is locked at advertise time. If it changes underneath
    us the frame is skipped rather than sent: the advertised `joint_count` is
    what tells a consumer how to read the array at all, so a length that drifts
    from it is a schema break, not a value to publish.
    """
    period = 1.0 / max(rate_hz, 1)
    expected = len(urdf_names)

    def sample():
        current = ctx.galbot.get_joint_names()
        if current and list(current) != list(sdk_names):
            return None
        positions = ctx.galbot.get_joint_positions(list(sdk_names))
        if positions is None:
            return None
        return angles_in_urdf_order(urdf_names, list(sdk_names), list(positions))

    def source():
        import asyncio

        from auki_datatypes.auki.joint_encoders import Data as JointData

        async def pull():
            while True:
                angles = sample()
                if angles is not None and len(angles) == expected:
                    yield {"timestamp_ns": time.time_ns(),
                           "payload": bytes(JointData(angles_rad=angles))}
                await asyncio.sleep(period)

        return pull()

    return source


def scaled_camera_size(width, height, max_edge=MAX_CAMERA_EDGE):
    """Pixel size after the long-edge cap. A frame already within it stays."""
    width = int(width)
    height = int(height)
    long_edge = max(width, height)
    if long_edge <= max_edge or long_edge < 1:
        return width, height

    def along(value):
        return max(1, (value * max_edge + long_edge // 2) // long_edge)

    return along(width), along(height)


def _resize_bgr(frame, width, height):
    import cv2

    return cv2.resize(
        frame, (int(width), int(height)), interpolation=cv2.INTER_AREA)


def scale_bgr_frame(frame, max_edge=MAX_CAMERA_EDGE):
    """`frame`, or a smaller copy whose long edge is at most `max_edge`."""
    size = _bgr_size(frame)
    if size is None:
        return None
    width, height = scaled_camera_size(size[0], size[1], max_edge)
    if (width, height) == size:
        return frame
    return _resize_bgr(frame, width, height)


def encode_bgr_jpeg(frame, quality):
    """JPEG bytes for a BGR frame, or None when there is nothing to encode."""
    if frame is None:
        return None
    import cv2

    ok, encoded = cv2.imencode(
        ".jpg", frame, [int(cv2.IMWRITE_JPEG_QUALITY), int(quality)])
    if not ok:
        return None
    return encoded.tobytes()


def _bgr_size(frame):
    shape = getattr(frame, "shape", None)
    if shape is None or len(shape) < 2:
        return None
    height, width = int(shape[0]), int(shape[1])
    if width < 1 or height < 1:
        return None
    return width, height


def _encode_camera_payload(jpeg):
    """CameraFrame protobuf. Field 2 is the JPEG Peyote decodes."""
    if not jpeg:
        return None
    try:
        import auki_sdk
        encode = getattr(auki_sdk, "encode_camera_frame_image", None)
        if callable(encode):
            return bytes(encode(jpeg))
    except Exception:  # noqa: BLE001
        pass
    try:
        from auki_datatypes.auki.camera import CameraFrame
        return bytes(CameraFrame(frame=jpeg))
    except Exception:  # noqa: BLE001
        return None


def make_camera_source(ctx, rate_hz, quality):
    """head_left JPEGs. A missing frame is a skipped tick, not stream end."""
    period = 1.0 / max(rate_hz, 1)

    def source():
        import asyncio

        async def pull():
            while True:
                payload = None
                try:
                    frame = scale_bgr_frame(ctx.galbot.get_rgb_image(HEAD_LEFT_CAMERA))
                    payload = _encode_camera_payload(encode_bgr_jpeg(frame, quality))
                except Exception as error:  # noqa: BLE001
                    log.warning("head_left camera frame failed: %s", error)
                if payload:
                    yield {"timestamp_ns": time.time_ns(),
                           "payload": payload}
                await asyncio.sleep(period)

        return pull()

    return source


# ----------------------------------------------------------- the declarations

def build(ctx):
    """Declare what this robot can serve, and bind each to its producer.

    Returns `(resources, device_model)`. Each resource is independently
    optional: a robot with no URDF still publishes its pose, one whose
    joint set cannot be probed still publishes both the pose and its model,
    and one whose head_left probe produces no JPEG omits the camera.

    `transport`/`endpoint` are required by the protocol and describe the
    cross-process shape. Nothing reads them here -- the peer is in this
    process -- but they stay accurate for a node that discovers this robot
    over HTTP.
    """
    resources = []

    # Component mode publishes robot-local odometry plus an explicit alignment.
    if os.environ.get("GALBOT_ODOMETRY_COMPONENT") != "1":
        pose_fps = _fps("AUKI_POSE_FPS", DEFAULT_POSE_FPS)
        resources.append(LiveResource(
            p.Resource(
                resource_id=POSE_RESOURCE_ID,
                payload="spatial_transform",
                rate_hz=pose_fps,
                from_frame=p.Frame(frame_id=BASE_FRAME_ID, convention=WIRE_CONVENTION),
                to_frame=p.Frame(frame_id=DOMAIN_FRAME_ID, convention=WIRE_CONVENTION),
                writer_mode="movable",
                transport="unix",
                endpoint="/run/auki/galbot/pose.sock"),
            make_pose_source(ctx, pose_fps)))

    urdf_path = robot_urdf_path()
    device_model = None
    if urdf_path is None:
        log.info("AUKI_ROBOT_URDF unset or missing; not advertising a device "
                 "model, and joint encoders need it for their wire order")
    else:
        device_model, advertise_path = _device_model(urdf_path)
        joints = _joint_resource(ctx, advertise_path)
        if joints is not None:
            resources.append(joints)

    if os.environ.get("GALBOT_SENSOR_COMPONENTS") != "1":
        camera = _camera_resource(ctx)
        if camera is not None:
            resources.append(camera)

    return resources, device_model


def _device_model(urdf_path):
    """Advertise the URDF, with Draco substitutions when the cache can build them.

    Returns `(model, urdf_path)`. The path is the rewritten advertise URDF when
    visual OBJs were pointed at convex-hull STLs, otherwise the caller's path.
    Joint order is read from the same file.
    """
    advertise_path = urdf_path
    substitutions = None
    try:
        prepared = display_meshes.prepare_advertise(
            urdf_path, state_dir=config.state_dir_path())
    except (OSError, ValueError) as error:
        log.warning("display mesh prepare failed for %s: %s; "
                    "advertising URDF as-is", urdf_path, error)
    else:
        advertise_path = prepared.urdf_path
        substitutions = prepared.substitutions
    if substitutions is None:
        try:
            substitutions = urdf_manifest.load_mesh_substitutions(advertise_path)
        except (OSError, ValueError, json.JSONDecodeError) as error:
            log.warning("manifest/substitutions failed for %s: %s; "
                        "advertising URDF meshes as-is", advertise_path, error)
            substitutions = None
    kwargs = {"urdf_path": advertise_path, "convention": "ros_body"}
    if substitutions:
        kwargs["mesh_substitutions"] = substitutions
    try:
        return p.DeviceModel(**kwargs), advertise_path
    except TypeError:
        if "mesh_substitutions" not in kwargs:
            raise
        log.info("DeviceModel has no mesh_substitutions; advertising URDF "
                 "meshes as-is (rebuild robot-runner-kit / auki_robot_node)")
        kwargs.pop("mesh_substitutions")
        return p.DeviceModel(**kwargs), advertise_path


def _joint_resource(ctx, urdf_path):
    try:
        urdf_names = urdf_active_joint_names(urdf_path)
    except OSError as error:
        log.warning("could not read %s for joint order: %s", urdf_path, error)
        return None
    if not urdf_names:
        log.warning("%s declares no revolute or continuous joints; not "
                    "advertising joint encoders", urdf_path)
        return None
    try:
        sdk_names = ctx.galbot.get_joint_names()
    except Exception as error:  # noqa: BLE001
        log.warning("could not probe joint names: %s", error)
        return None
    if not sdk_names:
        log.warning("the robot reports no joints; not advertising joint encoders")
        return None
    if not set(urdf_names) & set(sdk_names):
        log.warning("no overlap between %d URDF joints and %d SDK joints; not "
                    "advertising joint encoders", len(urdf_names), len(sdk_names))
        return None

    joint_fps = _fps("AUKI_JOINT_FPS", DEFAULT_JOINT_FPS)
    return LiveResource(
        p.Resource(
            resource_id=JOINTS_RESOURCE_ID,
            payload="joint_encoders_frame",
            rate_hz=joint_fps,
            sensor=p.Sensor(kind="joint_encoders",
                            body={"type": "absolute",
                                  "joint_count": len(urdf_names)}),
            # The same wire frame the pose names, and necessarily the same
            # convention: the protocol refuses a frame id two resources
            # describe differently.
            frame=p.Frame(frame_id=BASE_FRAME_ID, convention=WIRE_CONVENTION),
            transport="unix",
            endpoint="/run/auki/galbot/joints.sock"),
        make_joint_source(ctx, joint_fps, urdf_names, list(sdk_names)))


def _camera_resource(ctx):
    """head_left as a live camera_frame, or None when the probe cannot encode."""
    get_rgb = getattr(getattr(ctx, "galbot", None), "get_rgb_image", None)
    if not callable(get_rgb):
        log.info("head_left camera is not available; not advertising %s",
                 CAMERA_RESOURCE_ID)
        return None
    quality = _jpeg_quality("AUKI_CAMERA_JPEG_QUALITY", DEFAULT_CAMERA_JPEG_QUALITY)
    camera_fps = _fps("AUKI_CAMERA_FPS", DEFAULT_CAMERA_FPS)
    try:
        frame = scale_bgr_frame(get_rgb(HEAD_LEFT_CAMERA))
        size = _bgr_size(frame)
        jpeg = encode_bgr_jpeg(frame, quality) if size else None
    except Exception as error:  # noqa: BLE001
        log.warning("could not probe head_left camera: %s", error)
        return None
    if not jpeg or size is None:
        log.warning("head_left camera produced no JPEG; not advertising %s",
                    CAMERA_RESOURCE_ID)
        return None
    width, height = size
    return LiveResource(
        p.Resource(
            resource_id=CAMERA_RESOURCE_ID,
            payload="camera_frame",
            rate_hz=camera_fps,
            sensor=p.Sensor(
                kind="camera",
                body={
                    "type": "rgb",
                    "width": width,
                    "height": height,
                    "frame_rate_hz": camera_fps,
                    "image_encoding": "jpeg",
                    "pixel_format": "rgb8",
                    "row_stride_bytes": 0,
                    "color_space": "srgb",
                    "intrinsics_model": "pinhole",
                    "distortion_model": "none",
                },
            ),
            frame=p.Frame(frame_id=BASE_FRAME_ID, convention=WIRE_CONVENTION),
            transport="unix",
            endpoint="/run/auki/galbot/camera.sock"),
        make_camera_source(ctx, camera_fps, quality))
