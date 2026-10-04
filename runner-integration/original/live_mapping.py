"""Galbot sensor/legacy-data adapter for the native Component mapping pipeline.

Native SDK owns detection, localization, PnP, pose history, frame composition,
portal placement and peer publication. No latest-navigation-pose fallback.
"""
import json
import logging
import os
import threading
import time
from pathlib import Path

import numpy as np
from scipy.spatial.transform import Rotation

from portal_mapping import resolve_portal_metadata, rigid
from utils.transforms import convert_pose_to_transform

log = logging.getLogger(__name__)
DOMAIN_TO_ZUP = np.array([[1,0,0,0], [0,0,-1,0], [0,1,0,0], [0,0,0,1]], dtype=float)


def labelled(matrix, source, target):
    matrix = rigid(matrix)
    x,y,z,w = Rotation.from_matrix(matrix[:3,:3]).as_quat()
    return dict(from_frame_id=source, to_frame_id=target,
                translation=matrix[:3,3].tolist(), rotation_wxyz=[w,x,y,z])


def decode_camera_frame(frame):
    import cv2
    if frame.format.lower() != 'nv12':
        from utils.sdk import decode_compressed_RgbData
        return decode_compressed_RgbData(frame)
    width, height = int(frame.width), int(frame.height)
    if width <= 0 or height <= 0 or width % 2 or height % 2 or width*height > 16_777_216:
        raise ValueError('invalid NV12 dimensions')
    if frame.plane_count != 2:
        raise ValueError('NV12 requires two declared planes')
    data = frame.data
    planes = []
    for i, rows in enumerate((height, height//2)):
        offset, stride = int(frame.plane_offset_bytes[i]), int(frame.stride_bytes[i])
        if offset < 0 or stride < width or offset+(rows-1)*stride+width > len(data):
            raise ValueError('NV12 plane exceeds its declared buffer')
        planes.append(np.ndarray((rows,width), dtype=np.uint8, buffer=data, offset=offset, strides=(stride,1)))
    return cv2.cvtColor(np.vstack(planes), cv2.COLOR_YUV2BGR_NV12)


def validate_optical_frame(image_frame, calibration_frame, expected):
    declarations = [f for f in (image_frame, calibration_frame) if f]
    if not declarations or any(f != expected for f in declarations):
        raise ValueError('missing or conflicting camera optical frame declaration')


def rectification_maps(width, height, k, d, model):
    import cv2
    name = model.lower().replace('_', ' ').replace('-', ' ')
    if not np.isfinite(k).all() or not np.isfinite(d).all() or k[0,0] <= 0 or k[1,1] <= 0:
        raise ValueError('invalid camera intrinsics')
    if name in ('inverse brown conrady', 'modified brown conrady'):
        if d.size != 5:
            raise ValueError('RealSense Brown-Conrady requires five coefficients')
        # Ideal rays -> source pixels, matching librealsense's declared models:
        # https://github.com/realsenseai/librealsense/blob/master/src/rs.cpp
        u, v = np.meshgrid(np.arange(width, dtype=float), np.arange(height, dtype=float))
        x, y = (u-k[0,2])/k[0,0], (v-k[1,2])/k[1,1]
        radius = x*x + y*y
        radial = 1 + d[0]*radius + d[1]*radius**2 + d[4]*radius**3
        x, y = x*radial, y*radial
        dx = x + 2*d[2]*x*y + d[3]*(radius+2*x*x)
        dy = y + 2*d[3]*x*y + d[2]*(radius+2*y*y)
        return (dx*k[0,0]+k[0,2]).astype(np.float32), (dy*k[1,1]+k[1,2]).astype(np.float32)
    if name in ('plumb bob', 'brown conrady', 'rational polynomial', 'none') or not np.any(d):
        if name == 'none' and np.any(d):
            raise ValueError('nonzero distortion coefficients for an undistorted camera')
        return cv2.initUndistortRectifyMap(k, d, None, k, (width, height), cv2.CV_32FC1)
    raise ValueError('unsupported declared camera distortion model: ' + model)


def reference_anchors(ctx, domain, map_frame):
    anchors = []
    for record in ctx.domain_client.list_poses():
        short = record['short_id']
        metadata = json.loads(resolve_portal_metadata(ctx, domain, short))
        portal = metadata['id']
        size = float(metadata['size']) / 100.0
        if not np.isfinite(size) or size <= 0:
            raise ValueError('invalid canonical portal size')
        pose = [record[k] for k in ('px','py','pz','rx','ry','rz','rw')]
        anchors.append(dict(anchor_id=portal, payload='HTTPS://R8.HR/' + short,
            side_length_m=size, pose_in_map=labelled(DOMAIN_TO_ZUP @ convert_pose_to_transform(pose),
                'portal-' + portal, map_frame)))
    if not anchors:
        raise ValueError('domain has no reference portals for calibration')
    return anchors


class LiveMapping:
    def __init__(self, ctx, publisher, contract, publication_clock, root):
        self.ctx, self.publisher, self.contract = ctx, publisher, contract
        self.publication_clock = publication_clock
        self.root = Path(root)
        self.stop = threading.Event()
        self.pipeline = None
        self.state = dict(status='starting', portal_count=0, clock=contract['clock'])
        self.thread = threading.Thread(target=self.run, name='component-portal-mapping', daemon=True)
        self.thread.start()

    def write(self, **updates):
        self.state.update(updates)
        self.root.mkdir(parents=True, exist_ok=True)
        path = self.root / 'mapping-status.json'
        tmp = path.with_suffix('.tmp')
        tmp.write_text(json.dumps(self.state, allow_nan=False, indent=2))
        tmp.replace(path)

    def read(self):
        import cv2
        self.write(stage='acquiring_camera')
        from types import SimpleNamespace
        packet = self.ctx.camera_captures.mapping(self.state['camera'])
        if packet is None:
            self.write(stage='waiting_for_camera', camera_wait_reason='no synchronized frame')
            return None
        raw_frame = dict(packet['frame'])
        raw_frame['header'] = SimpleNamespace(**raw_frame['header'])
        frame = SimpleNamespace(**raw_frame)
        timestamp = int(frame.header.timestamp_ns)
        if timestamp <= 0:
            raise ValueError('invalid camera measurement timestamp')
        if timestamp == self.last:
            self.write(stage='waiting_for_camera', camera_wait_reason='repeated capture timestamp', capture_timestamp_ns=timestamp)
            return None
        if self.last is not None and timestamp < self.last:
            raise ValueError('camera clock regressed; new session required')
        self.write(stage='reading_intrinsics', capture_timestamp_ns=timestamp, camera_wait_reason=None)
        info = packet['intrinsics']
        validate_optical_frame(str(frame.header.frame_id), info.get('header', {}).get('frame_id'), self.optical)
        self.write(stage='reading_capture_transform')
        pose, pose_time = packet['camera_to_base'], packet['transform_timestamp_ns']
        if not pose or int(pose_time) != timestamp:
            self.write(status='waiting_for_capture_transform', capture_timestamp_ns=timestamp)
            return None
        k = np.asarray(info['K'], dtype=float).reshape(3,3)
        d = np.asarray(info['D'], dtype=float)
        self.write(stage='decoding_camera')
        image = decode_camera_frame(frame)
        if image is None:
            return None
        model = str(info.get('distortion_model', ''))
        spec = (image.shape, k.tobytes(), d.tobytes(), model)
        if getattr(self, 'rectification_spec', None) is None:
            self.rectification = rectification_maps(image.shape[1], image.shape[0], k, d, model)
            self.rectification_spec = spec
        elif spec != self.rectification_spec:
            raise ValueError('camera calibration changed; new session required')
        image = cv2.remap(image, *self.rectification, interpolation=cv2.INTER_LINEAR)
        if self.intrinsics is not None and (not np.array_equal(k, self.intrinsics) or not np.array_equal(d, self.distortion)):
            raise ValueError('camera calibration changed; new session required')
        self.last = timestamp
        return timestamp, labelled(convert_pose_to_transform(pose), self.optical, self.contract['from_frame']['frame_id']), cv2.cvtColor(image, cv2.COLOR_BGR2RGB), k, d

    def mount(self, image, k, d):
        import auki_registry as registry
        owner = self.contract['clock']['peer_id']
        domain = self.ctx.domain_client.assigned_domain_id
        session = self.contract['session_id']
        map_frame = 'domain.' + domain + '.zup'
        frame = dict(peer_id=owner, frame_id=map_frame, units='meters', handedness='right',
                     axes=dict(x='right', y='forward', z='up'))
        registry.write_frame(str(self.root), frame)
        optical = registry.frame_ros_optical(owner, self.optical)
        optical.pop('hash', None)
        registry.write_frame(str(self.root), optical)
        config = dict(domain_id=domain, map_id='galbot-' + session, map_frame=map_frame,
                      optical_frame=self.optical, optical_registry=optical, capture_clock=self.contract['clock'],
                      publication_clock=self.publication_clock, width=int(image.shape[1]), height=int(image.shape[0]),
                      fx=float(k[0,0]), fy=float(k[1,1]), cx=float(k[0,2]), cy=float(k[1,2]))
        anchors = reference_anchors(self.ctx, domain, map_frame)
        self.pipeline = self.publisher.mount_mapping(json.dumps(config), json.dumps(dict(self.contract, to_frame=frame)),
                    lambda dom, portal: resolve_portal_metadata(self.ctx, dom, portal))
        self.pipeline.seed_reference(json.dumps(anchors, allow_nan=False))
        self.intrinsics, self.distortion, self.shape = k.copy(), d.copy(), image.shape
        self.write(status='waiting_for_portal', reference_portals=len(anchors), map_frame=map_frame,
                   map_id=config['map_id'], clock=self.contract['clock'])

    def at_capture(self, call, timestamp):
        deadline = time.monotonic() + .5
        while not self.stop.is_set():
            try:
                return json.loads(call())
            except RuntimeError as exc:
                if not any(reason in str(exc) for reason in (
                        'outside retained history', 'maximum gap', 'tracking is lost')):
                    raise
                if time.monotonic() >= deadline:
                    self.write(status='waiting_for_pose', capture_timestamp_ns=timestamp,
                               pose_skips=self.state.get('pose_skips', 0) + 1, timing_error=str(exc))
                    return None
                self.stop.wait(.02)
        return None

    def run(self):
        try:
            from utils.sdk import galbot_sensor_types, galbot_sensor_link_names
            camera = os.environ.get('GALBOT_MAPPING_CAMERA', 'left_arm')
            if camera not in ('left_arm', 'right_arm'):
                raise ValueError('GALBOT_MAPPING_CAMERA must be left_arm or right_arm')
            self.sensor = galbot_sensor_types[camera]
            self.optical = galbot_sensor_link_names[camera]
            self.write(camera=camera, optical_frame=self.optical)
            self.last = None
            self.intrinsics = None
            aligned = False
            self.write(status='waiting_for_camera')
            while not self.stop.is_set():
                sample = self.read()
                if sample is None:
                    self.stop.wait(.2)
                    continue
                timestamp, camera_base, image, k, d = sample
                if self.pipeline is None:
                    self.mount(image, k, d)
                if image.shape != self.shape:
                    raise ValueError('camera resolution changed; new session required')
                clock = json.dumps(self.contract['clock'])
                if not aligned:
                    try:
                        self.write(stage='localizing')
                        result = json.loads(self.pipeline.localize(image.tobytes(), timestamp))
                    except RuntimeError as exc:
                        if not any(reason in str(exc) for reason in (
                                'pose solver failed', 'pose is behind the camera',
                                'corners are outside the image')):
                            raise
                        self.write(status='waiting_for_portal', capture_timestamp_ns=timestamp,
                                   localization_rejection=str(exc))
                        self.stop.wait(1.)
                        continue
                    self.write(stage='localization_complete', status='waiting_for_portal', detected_codes=result.get('detected_codes', 0), seen_portal_ids=result.get('seen_portal_ids', []), capture_timestamp_ns=timestamp)
                    if 'camera_to_map' in result:
                        # SDK buffer lookup interpolates only between bracketing samples;
                        # slow detector work does not retimestamp the original capture.
                        alignment = self.at_capture(lambda: self.publisher.align(timestamp, clock, json.dumps(camera_base), json.dumps(result['camera_to_map'])), timestamp)
                        if alignment is None:
                            continue
                        aligned = True
                        self.write(status='mapping', alignment=alignment, calibration=result['localization'])
                        log.info('Component map calibrated at measurement %s', timestamp)
                else:
                    # A recent image may arrive just ahead of the next odometry poll.
                    # Wait briefly for a bracket; never extrapolate or use latest pose.
                    capture = self.at_capture(lambda: self.publisher.camera_in_map(timestamp, clock, json.dumps(camera_base)), timestamp)
                    if capture is None:
                        continue
                    try:
                        result = json.loads(self.pipeline.process(image.tobytes(), timestamp, json.dumps(capture['pose'])))
                    except RuntimeError as exc:
                        if 'solver failed' not in str(exc):
                            raise
                        self.write(status='mapping', capture_timestamp_ns=timestamp, placement_rejection=str(exc))
                        self.stop.wait(1.)
                        continue
                    tmp = self.root / 'observed-portals.usda.tmp'
                    tmp.write_text(result['inspection_usda'])
                    tmp.replace(self.root / 'observed-portals.usda')
                    self.write(status='mapping', portal_count=len(result['snapshot']['scenegraph']['anchors']),
                               snapshot_reference=result['snapshot_reference'], last_capture=capture,
                               unresolved_portals=result.get('unresolved_portals', []))
                self.stop.wait(1.)
        except Exception as exc:
            log.exception('Component mapping stopped')
            self.write(status='failed', error=str(exc))
        finally:
            if self.pipeline is not None:
                self.pipeline.close()

    def close(self):
        self.stop.set()
        self.thread.join()
