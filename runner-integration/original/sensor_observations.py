"""Read-only Galbot adapters; SDK Components own publication, history and transport."""
import base64
import json
import math
import threading
import time
from pathlib import Path

CAMERAS = ('head_left', 'head_right', 'left_arm', 'right_arm',
           'front_left', 'front_right', 'rear_left', 'rear_right')


def timestamp(value):
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise ValueError('missing/invalid vendor measurement timestamp')
    return value


def vector(value):
    result = [float(value[k]) for k in ('x', 'y', 'z')]
    if not all(math.isfinite(x) for x in result):
        raise ValueError('non-finite sensor vector')
    return result


def binary(value):
    return base64.b64encode(bytes(value)).decode('ascii')


def header(data, fallback=None):
    h = data['header']
    t = timestamp(h['timestamp_ns'])
    frame = h.get('frame_id') or fallback
    if not frame:
        raise ValueError('sensor has no declared frame')
    if fallback and h.get('frame_id') and h['frame_id'] != fallback:
        raise ValueError('sensor and calibration frame declarations disagree')
    return t, str(frame)


def point_cloud(data):
    width, height = int(data['width']), int(data['height'])
    step, row = int(data['point_step']), int(data['row_step'])
    raw = bytes(data['data'])
    if min(width, height, step) <= 0 or row < width * step or len(raw) != row * height:
        raise ValueError('invalid point cloud layout')
    sizes = {1: 1, 2: 1, 3: 2, 4: 2, 5: 4, 6: 4, 7: 4, 8: 8}
    fields = []
    for field in data['fields']:
        f = dict(name=str(field['name']), offset=int(field['offset']),
                 datatype=int(field['datatype']), count=int(field['count']))
        if f['count'] <= 0 or f['offset'] < 0 or f['offset'] + sizes[f['datatype']] * f['count'] > step:
            raise ValueError('point field exceeds point stride')
        fields.append(f)
    if not {'x', 'y', 'z'} <= {f['name'] for f in fields}:
        raise ValueError('point cloud lacks XYZ fields')
    return dict(kind='point_cloud', width=width, height=height, point_step=step,
                row_step=row, is_bigendian=bool(data['is_bigendian']),
                is_dense=bool(data['is_dense']), fields=fields, data_base64=binary(raw))


def depth(data):
    w, h, scale = int(data['width']), int(data['height']), float(data['depth_scale'])
    if min(w, h) <= 0 or not math.isfinite(scale) or scale <= 0:
        raise ValueError('invalid depth geometry/scale')
    encoding = str(data['format'])
    raw = bytes(data['data'])
    if encoding == '16UC1' and len(raw) != w * h * 2:
        raise ValueError('depth buffer dimensions mismatch')
    if not raw:
        raise ValueError('empty depth frame')
    return dict(kind='depth', width=w, height=h, encoding=encoding,
                counts_per_meter=scale, data_base64=binary(raw))


def camera_preview(raw, width, height, calibration):
    """Resize a display-only JPEG, preserving the calibrated optical geometry."""
    import cv2
    import numpy as np
    if (int(calibration.get('width', 0)), int(calibration.get('height', 0))) != (width, height):
        raise ValueError('preview calibration dimensions differ from source image')
    k = np.asarray(calibration['K'], dtype=float)
    if k.size != 9 or not np.isfinite(k).all():
        raise ValueError('preview requires a finite 3x3 camera matrix')
    image = cv2.imdecode(np.frombuffer(raw, dtype=np.uint8), cv2.IMREAD_COLOR)
    if image is None or image.shape[:2] != (height, width):
        raise ValueError('preview JPEG dimensions differ from source contract')
    scale = min(1.0, 640.0 / max(width, height))
    w, h = max(1, round(width * scale)), max(1, round(height * scale))
    image = cv2.resize(image, (w, h), interpolation=cv2.INTER_AREA)
    sx, sy = w / width, h / height
    # OpenCV resize maps pixel centers, including the half-pixel offset.
    transform = np.array([[sx, 0, (sx - 1) / 2], [0, sy, (sy - 1) / 2], [0, 0, 1]])
    adjusted = dict(calibration, width=w, height=h,
                    K=(transform @ k.reshape(3, 3)).reshape(k.shape).tolist())
    ok, jpeg = cv2.imencode('.jpg', image, [cv2.IMWRITE_JPEG_QUALITY, 55])
    if not ok:
        raise ValueError('preview JPEG encoding failed')
    return jpeg.tobytes(), w, h, adjusted


class SensorObservations:
    def __init__(self, ctx, publisher, contract, root):
        import auki_registry as registry
        from utils.sdk import galbot_sensor_types
        from galbot_sdk import SensorType, RgbOutputFormat, GalbotOneFoxtrotSensor, UltrasonicType
        self.robot, self.publisher, self.contract = ctx.galbot.robot, publisher, contract
        self.cameras = ctx.camera_captures
        self.root = Path(root)
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.specs, self.last, self.counts, self.states = {}, {}, {}, {}
        self.failed = set()
        self.threads = []
        self.registry = registry
        self.owner = contract['clock']['peer_id']
        clock_id = 'galbot.sensor-read-complete.' + contract['session_id']
        clock = registry.utc_clock_entry(peer_id=self.owner, session_id=contract['session_id'],
            clock_id=clock_id, unit='nanoseconds', scope='device-local', epoch='1970-01-01T00:00:00Z')
        digest = registry.write_clock(str(self.root), clock)
        self.read_clock = dict(peer_id=self.owner, id=clock_id, hash=str(digest))
        for name in CAMERAS:
            sensor = galbot_sensor_types[name]
            self.start('camera.' + name, 5, lambda n=name, s=sensor: self.camera(n, s, RgbOutputFormat.JPEG))
        for name, sensor in [('left_arm', SensorType.LEFT_ARM_DEPTH_CAMERA), ('right_arm', SensorType.RIGHT_ARM_DEPTH_CAMERA)]:
            self.start('depth.' + name, 5, lambda n=name, s=sensor: self.depth(n, s))
        self.start('lidar.base', 10, lambda: self.lidar(SensorType.BASE_LIDAR))
        self.start('imu.chassis', 20, lambda: self.imu(SensorType.CHASSIS_IMU))
        self.start('joints', 20, self.joints)
        for name, sensor in GalbotOneFoxtrotSensor.__members__.items():
            self.start('wrench.' + name.lower(), 20, lambda n=name, s=sensor: self.wrench(n, s))
        for name, sensor in UltrasonicType.__members__.items():
            self.start('range.' + name.lower(), 10, lambda n=name, s=sensor: self.range(n, s))
        self.start('battery', 1, self.battery)
        self.start('status', 1, self.write_status)

    def start(self, name, hz, action):
        def run():
            while not self.stop.is_set():
                start = time.monotonic()
                try:
                    count = action()
                    with self.lock:
                        self.states[name] = dict(status='publishing' if count else 'unavailable_or_unchanged', poll_hz=hz)
                except Exception as exc:
                    with self.lock:
                        self.states[name] = dict(status='error', error=str(exc), poll_hz=hz)
                self.stop.wait(max(.001, 1 / hz - (time.monotonic() - start)))
        thread = threading.Thread(target=run, name='sensor-' + name, daemon=True)
        self.threads.append(thread)
        thread.start()

    def emit(self, identity, t, measurement=None, frame=None, camera=None, raw=b'', metadata=None, read_time=False, live_preview=False):
        t = timestamp(t)
        if identity in self.failed:
            raise ValueError("sensor requires a new session after clock/contract change")
        last = self.last.get(identity)
        if last is not None:
            if t < last:
                self.failed.add(identity)
                raise ValueError('sensor clock regressed; restart required')
            if t == last:
                return 0
        metadata = dict(metadata or {}, sensor_id=identity,
                        timestamp_basis='read_complete' if read_time else 'measurement',
                        retention_seconds=1 if live_preview else 10)
        if live_preview:
            metadata["retention_max_entries"] = 1
        if camera:
            payload = dict(kind='camera', datatype='video_frame', schema='auki.video-frame/v1',
                           encoding='jpeg', width=camera[0], height=camera[1],
                           nominal_frame_rate_hz=5 if live_preview else None,
                           observes=metadata.get("preview_for", identity))
        else:
            payload = dict(kind='structured', datatype='galbot.sensor-reading', schema='galbot.sensor-reading/v1',
                           modality=measurement['kind'], observes=identity, unit=None)
        definition = None
        if frame:
            optical = camera is not None or measurement['kind'] == 'depth'
            builder = self.registry.frame_ros_optical if optical else self.registry.frame_ros_body
            definition = builder(self.owner, frame)
            definition.pop('hash', None)
            # Persist the existing canonical registry entry, not a second frame schema.
            if identity not in self.specs:
                self.registry.write_frame(str(self.root), definition)
        spec = dict(id=identity, clock=self.read_clock if read_time else self.contract['clock'],
                    frame_definition=definition, payload=payload, metadata=metadata, live_preview=live_preview)
        encoded = json.dumps(spec, sort_keys=True, allow_nan=False)
        if identity not in self.specs:
            self.publisher.register_sensor(encoded)
            self.specs[identity] = encoded
        elif self.specs[identity] != encoded:
            self.failed.add(identity)
            raise ValueError('sensor contract changed; restart required')
        data = '' if camera else json.dumps(dict(sensor_id=identity, measurement=measurement), allow_nan=False)
        self.publisher.publish_sensor(identity, t, data, raw)
        self.last[identity] = t
        with self.lock:
            self.counts[identity] = self.counts.get(identity, 0) + 1
        return 1

    def camera(self, name, sensor, jpeg):
        data, info = self.cameras.rgb(name)
        if not data:
            return 0
        t, frame = header(data, info.get('header', {}).get('frame_id'))
        w, h = int(data['width']), int(data['height'])
        if str(data['format']).lower() not in ('jpeg', 'jpg'):
            raise ValueError('vendor did not return requested JPEG')
        # Calibration metadata is timeless; do not embed its changing header timestamp.
        calibration = {k: info[k] for k in ('K', 'D', 'distortion_model', 'width', 'height') if k in info}
        raw = bytes(data['data'])
        source = 'camera.' + name
        count = self.emit(source, t, frame=frame, camera=(w, h), raw=raw,
                          metadata=dict(calibration=calibration, image_geometry='vendor_original'))
        if count:
            preview = source + '-preview'
            try:
                jpeg_bytes, pw, ph, intrinsics = camera_preview(raw, w, h, calibration)
                self.emit(preview, t, frame=frame, camera=(pw, ph), raw=jpeg_bytes,
                    live_preview=True, metadata=dict(preview_for=source,
                        source_product_id=source + '-history', calibration=intrinsics,
                        image_geometry='resized_pixel_centers', source_width=w,
                        source_height=h, jpeg_quality=55))
                with self.lock:
                    self.states[preview] = dict(status='publishing', poll_hz=5)
            except Exception as exc:
                # Display processing must not interrupt full-resolution capture.
                with self.lock:
                    self.states[preview] = dict(status='error', error=str(exc), poll_hz=5)
        return count

    def depth(self, name, sensor):
        data = self.robot.get_depth_data(sensor)
        if not data:
            return 0
        t, frame = header(data)
        return self.emit('depth.' + name, t, depth(data), frame,
                         metadata=dict(invalid_value=0, distance_unit='meters', scaling='depth_m = sample / counts_per_meter'))

    def lidar(self, sensor):
        data = self.robot.get_lidar_data(sensor)
        if not data:
            return 0
        t, frame = header(data)
        # Preserve every vendor point field verbatim, including per-point timing fields.
        # Timing fields remain opaque vendor attributes until their clock semantics are declared.
        return self.emit('lidar.base', t, point_cloud(data), frame,
                         metadata=dict(position_unit='meters', field_datatypes='ROS PointField',
                                       point_time_fields='opaque vendor attributes; not absolute timestamps'))

    def imu(self, sensor):
        data = self.robot.get_imu_data(sensor)
        if not data:
            return 0
        # This is the named G1 chassis IMU source, not a pose or alignment to base_link.
        return self.emit('imu.chassis', timestamp(data['timestamp_ns']),
                         dict(kind='imu', acceleration_m_s2=vector(data['accel']),
                              angular_velocity_rad_s=vector(data['gyro'])), 'chassis_imu_link')

    def joints(self):
        total = 0
        for joint in self.robot.get_joint_states():
            name = str(joint.joint_name)
            if not name:
                raise ValueError('unnamed joint')
            total += self.emit('joint.' + name, timestamp(joint.timestamp_ns),
                dict(kind='joint', name=name, position_rad=float(joint.position),
                     velocity_rad_s=float(joint.velocity), effort_nm=float(joint.effort),
                     current_a=float(joint.current), acceleration_rad_s2=float(joint.acceleration)))
        return total

    def wrench(self, name, sensor):
        data = self.robot.get_force_sensor_data(sensor, False, '')
        if not data:
            return 0
        # Explicit source-frame mapping documented by get_force_sensor_data.
        frame = ('left' if name.startswith('LEFT') else 'right') + '_arm_force_sensor_joint'
        return self.emit('wrench.' + name.lower(), timestamp(data['timestamp_ns']),
                         dict(kind='wrench', force_n=vector(data['force']), torque_nm=vector(data['torque'])), frame)

    def range(self, name, sensor):
        data = self.robot.get_ultrasonic_data(sensor)
        if not data:
            return 0
        # Scalar distance carries sensor identity; no invented spatial pose.
        return self.emit('range.' + name.lower(), timestamp(data['timestamp_ns']),
                         dict(kind='range', distance_m=float(data['distance'])))

    def battery(self):
        data = self.robot.get_bms_information()
        if not data:
            return 0
        return self.emit('battery', time.time_ns(), dict(kind='battery',
            voltage_v=float(data['voltage']), current_a=float(data['current']),
            level_pct=float(data['battery_level']), temperature_c=float(data['temperature']),
            charging=bool(data['charging_status']), healthy=bool(data['health_status']),
            capacity_ah=float(data['capacity'])), read_time=True)

    def write_status(self):
        with self.lock:
            value = dict(sources=dict(self.states), published=dict(self.counts), retention_seconds=10)
        self.root.mkdir(parents=True, exist_ok=True)
        path = self.root / 'sensors-status.json'
        temporary = path.with_suffix('.tmp')
        temporary.write_text(json.dumps(value, allow_nan=False, indent=2))
        temporary.replace(path)
        return 1

    def close(self):
        self.stop.set()
        for thread in self.threads:
            thread.join()
