#!/usr/bin/env python3
"""Read-only Galbot D405 RGB-D source. No controller or navigation APIs.

The SDK maps both D405 sensor enums to the color optical TF. Require identical
intrinsics and identity depth-to-color extrinsic, and use RealSense deprojection
for the reported distortion model. Publish bounded sensor-local XYZ + sRGB.
"""
import hashlib
import json
import math
import os
import signal
import sys
import time

FRAME = 'right_arm_camera_color_optical_frame'


def calibration(robot, rgb_sensor, depth_sensor, sensor_frame=FRAME):
    if sensor_frame not in (FRAME, 'left_arm_camera_color_optical_frame'):
        raise ValueError('unsupported mapping camera frame')
    rgb = robot.get_camera_intrinsic(rgb_sensor)
    depth = robot.get_camera_intrinsic(depth_sensor)
    keys = ['width', 'height', 'K', 'D', 'distortion_model', 'camera_type']
    if not all(k in rgb and k in depth for k in keys):
        raise ValueError('camera RGB/depth calibration unavailable')
    if any(rgb[k] != depth[k] for k in keys) or rgb['camera_type'] != 'D405':
        raise ValueError('RGB-D requires verified co-registered D405 intrinsics')
    if rgb['header']['frame_id'] != sensor_frame or depth['header']['frame_id'] != sensor_frame.replace('_color_', '_depth_'):
        raise ValueError('unexpected camera calibration frame')
    extrinsic, _ = robot.get_sensor_extrinsic(depth_sensor, sensor_frame)
    if len(extrinsic) != 7 or max(abs(a-b) for a,b in zip(extrinsic, [0,0,0,0,0,0,1])) > 1e-6:
        raise ValueError('RGB-D requires identity depth-to-color SDK extrinsic')
    stable = {k: depth[k] for k in keys}
    digest = hashlib.sha256(json.dumps(stable, sort_keys=True).encode()).hexdigest()
    return depth, 'Galbot SDK D405 co-registered RGB-D; RealSense deprojection; sha256:' + digest


def project(depth, rgb, info):
    import numpy as np
    import pyrealsense2 as rs
    if depth.shape != (info['height'], info['width']) or rgb.shape != (*depth.shape, 3):
        raise ValueError('RGB-D dimensions mismatch')
    intrinsic = rs.intrinsics()
    intrinsic.width, intrinsic.height = info['width'], info['height']
    intrinsic.fx, intrinsic.fy = info['K'][0], info['K'][4]
    intrinsic.ppx, intrinsic.ppy = info['K'][2], info['K'][5]
    models = {'Inverse Brown Conrady': rs.distortion.inverse_brown_conrady,
              'Brown Conrady': rs.distortion.brown_conrady, 'none': rs.distortion.none}
    if info['distortion_model'] not in models or len(info['D']) != 5:
        raise ValueError('unsupported depth distortion model')
    intrinsic.model, intrinsic.coeffs = models[info['distortion_model']], info['D']
    # Uniform 24x16 grid bounds raycasting cost and samples the whole image.
    points, colors = [], []
    for v in np.linspace(2, depth.shape[0]-3, 16).astype(int):
        for u in np.linspace(2, depth.shape[1]-3, 24).astype(int):
            z = float(depth[v, u])
            # D405 near-field range. Reject holes and uncertain depth boundaries.
            patch = depth[v-1:v+2, u-1:u+2]
            if not .1 <= z <= .5 or not np.isfinite(patch).all() or np.min(patch) <= 0 or np.ptp(patch) > .02:
                continue
            p = rs.rs2_deproject_pixel_to_point(intrinsic, [float(u), float(v)], z)
            if all(math.isfinite(x) for x in p):
                points.append(p)
                colors.append(rgb[v, u].tolist())
    if not points:
        raise ValueError('no valid D405 depth returns within 0.1–0.5 m')
    return points, colors


def synchronized_pair(robot, rgb_sensor, depth_sensor):
    # The SDK anchors on the first sensor and chooses nearest timestamps for
    # the rest. RGB arrives after depth on Echo: anchoring on fresh depth can
    # select the preceding RGB frame. Anchor on RGB, whose depth is available.
    observation = robot.get_synced_observation([rgb_sensor, depth_sensor], True)
    if observation is None:
        raise ValueError('RGB-D observation unavailable')
    rgb, depth = observation.rgb_data_map[rgb_sensor], observation.depth_data_map[depth_sensor]
    timestamp = int(depth.header.timestamp_ns)
    if timestamp <= 0 or int(rgb.header.timestamp_ns) != timestamp:
        raise ValueError('RGB/depth timestamps do not match exactly')
    return rgb, depth


def capture(robot, a, b, info, provenance, include_image=False, sensor_frame=FRAME):
    import numpy as np
    import cv2
    rgb, depth = synchronized_pair(robot, a, b)
    timestamp = int(depth.header.timestamp_ns)
    if str(depth.header.frame_id) != sensor_frame.replace('_color_', '_depth_') or str(rgb.header.frame_id) not in ('', sensor_frame):
        raise ValueError('unexpected capture frame')
    if depth.format != '16UC1' or depth.depth_scale <= 0 or rgb.format.lower() != 'nv12':
        raise ValueError('unsupported RGB-D encoding')
    w, h = int(rgb.width), int(rgb.height)
    if (w,h) != (info['width'],info['height']) or (depth.width,depth.height) != (w,h):
        raise ValueError('capture/calibration dimensions differ')
    if rgb.plane_count != 2 or list(rgb.stride_bytes) != [w,w] or list(rgb.plane_offset_bytes) != [0,w*h]:
        raise ValueError('unexpected NV12 plane layout')
    nv12 = np.frombuffer(rgb.data, np.uint8).reshape(h*3//2,w)
    color = cv2.cvtColor(nv12, cv2.COLOR_YUV2RGB_NV12)
    metres = np.frombuffer(depth.data, dtype='<u2').reshape(h,w).astype(float) / int(depth.depth_scale)
    pose, pose_time = robot.get_transform('base_link', sensor_frame, timestamp)
    if len(pose) != 7 or int(pose_time) != timestamp or not all(math.isfinite(x) for x in pose):
        raise ValueError('capture-time camera transform unavailable')
    if abs(sum(x*x for x in pose[3:])-1) > 1e-5:
        raise ValueError('invalid camera orientation')
    try:
        points, colors = project(metres, color, info)
    except ValueError:
        if not include_image:
            raise
        points, colors = [], []
    result = dict(timestamp_ns=timestamp, rgb_timestamp_ns=timestamp,
                transform_timestamp_ns=int(pose_time), hit_points_m=points,
                hit_colors_srgb=colors, calibration=dict(sensor_frame=sensor_frame,
                sensor_to_base=dict(translation=pose[:3], rotation_xyzw=pose[3:]),
                calibration_source=provenance))
    if include_image:
        import base64
        maps = rectification(info)
        gray = cv2.remap(nv12[:h], *maps, interpolation=cv2.INTER_LINEAR)
        result.update(width=w, height=h, fx=info['K'][0], fy=info['K'][4],
                      cx=info['K'][2], cy=info['K'][5], gray_base64=base64.b64encode(gray.tobytes()).decode())
    return result


_RECTIFICATION = {}
def rectification(info):
    """Ideal rays -> vendor image pixels, checked against RealSense's model."""
    import numpy as np
    import pyrealsense2 as rs
    key=json.dumps({k:info[k] for k in ['K','D','width','height','distortion_model']},sort_keys=True)
    if key in _RECTIFICATION:
        return _RECTIFICATION[key]
    if info['distortion_model'] != 'Inverse Brown Conrady':
        raise ValueError('unverified camera rectification model')
    k,d=info['K'],info['D']
    u,v=np.meshgrid(np.arange(info['width'],dtype=float),np.arange(info['height'],dtype=float))
    x,y=(u-k[2])/k[0],(v-k[5])/k[4]
    r=x*x+y*y
    f=1+d[0]*r+d[1]*r*r+d[4]*r*r*r
    x,y=x*f,y*f
    mx=(x+2*d[2]*x*y+d[3]*(r+2*x*x))*k[0]+k[2]
    my=(y+2*d[3]*x*y+d[2]*(r+2*y*y))*k[4]+k[5]
    intr=rs.intrinsics();intr.width=info['width'];intr.height=info['height']
    intr.fx,intr.fy,intr.ppx,intr.ppy=k[0],k[4],k[2],k[5]
    intr.coeffs=d;intr.model=rs.distortion.inverse_brown_conrady
    for yy in [0,info['height']//2,info['height']-1]:
        for xx in [0,info['width']//2,info['width']-1]:
            expected=rs.rs2_project_point_to_pixel(intr,[(xx-k[2])/k[0],(yy-k[5])/k[4],1.])
            if max(abs(expected[0]-mx[yy,xx]),abs(expected[1]-my[yy,xx])) > .002:
                raise ValueError('rectification differs from vendor projection')
    _RECTIFICATION[key]=(mx.astype(np.float32),my.astype(np.float32))
    return _RECTIFICATION[key]


def terminate(_signal, _frame):
    raise SystemExit(0)


def main():
    signal.signal(signal.SIGTERM, terminate)
    parent = os.getppid()
    from galbot_sdk.g1 import GalbotRobot, SensorType
    robot = GalbotRobot()
    a,b = SensorType.RIGHT_ARM_CAMERA,SensorType.RIGHT_ARM_DEPTH_CAMERA
    try:
        if not robot.init({a,b}, True):
            raise RuntimeError('read-only RGB-D initialization failed')
        time.sleep(3)
        info, provenance = calibration(robot,a,b)
        last = 0
        while os.getppid() == parent:
            started = time.monotonic()
            try:
                sample = capture(robot,a,b,info,provenance)
                if sample['timestamp_ns'] > last:
                    print('RGBD ' + json.dumps(sample, separators=(',',':')), flush=True)
                    last = sample['timestamp_ns']
                    if '--once' in sys.argv:
                        return
            except BrokenPipeError:
                return
            except Exception as error:
                print('RGBD capture: '+str(error), file=sys.stderr, flush=True)
            time.sleep(max(.01, 1-(time.monotonic()-started)))
    finally:
        robot.request_shutdown()
        robot.destroy()


if __name__ == '__main__':
    main()
