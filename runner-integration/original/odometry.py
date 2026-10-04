"""Galbot measurement adapter; native SDK owns Components and retained history."""
import asyncio
import json
import logging
import os
import threading
import time
import uuid
from pathlib import Path

log = logging.getLogger(__name__)


def tracking_from_sample(sample):
    """Preserve vendor measurement time, pose and quaternion order exactly."""
    if not sample:
        return None
    timestamp = sample['timestamp_ns']
    if isinstance(timestamp, bool) or not isinstance(timestamp, int) or timestamp < 0:
        raise ValueError('invalid odometry measurement timestamp')
    position, orientation = list(sample['position']), list(sample['orientation'])
    if len(position) != 3 or len(orientation) != 4:
        raise ValueError('invalid odometry pose dimensions')
    return timestamp, dict(status='valid', pose=dict(translation=position, rotation_xyzw=orientation))


def odometry_node_class(base, ctx):
    class OdometryEndpoint:
        def __init__(self, publisher, status_path):
            self.mapping = None
            self.sensors = None
            self.cameras = None
            self.publisher = publisher
            self.status_path = Path(status_path)
            self.stop = threading.Event()
            self.thread = threading.Thread(target=self.run, name='odometry-component', daemon=True)
            self.thread.start()

        def run(self):
            last = None
            last_new = time.monotonic()
            last_status = 0.0
            try:
                while not self.stop.is_set():
                    value = tracking_from_sample(ctx.galbot.robot.get_odom())
                    now = time.monotonic()
                    if value is not None:
                        timestamp, tracking = value
                        if last is not None and timestamp < last:
                            raise RuntimeError('odometry clock regressed; new frame/session required')
                        if timestamp != last:
                            self.publisher.publish(timestamp, json.dumps(tracking, allow_nan=False))
                            last, last_new = timestamp, now
                    if now - last_new > 5:
                        raise RuntimeError('odometry stalled; refusing to invent measurement timestamps')
                    if now - last_status >= 1:
                        self.publisher.refresh_control()
                        data = json.loads(self.publisher.status())
                        data['status'] = 'publishing' if last is not None else 'waiting'
                        self.write_status(data)
                        last_status = now
                    self.stop.wait(0.05)
            except Exception as exc:
                log.error('Odometry Component stopped: %s', exc)
                self.write_status(dict(status='failed', error=str(exc)))
            finally:
                self.publisher.close()

        def write_status(self, data):
            self.status_path.parent.mkdir(parents=True, exist_ok=True)
            temporary = self.status_path.with_suffix('.tmp')
            temporary.write_text(json.dumps(data, indent=2))
            temporary.replace(self.status_path)

        async def close(self):
            if self.sensors is not None:
                await asyncio.get_running_loop().run_in_executor(None, self.sensors.close)
            if self.mapping is not None:
                await asyncio.get_running_loop().run_in_executor(None, self.mapping.close)
            if self.cameras is not None:
                await asyncio.get_running_loop().run_in_executor(None, self.cameras.close)
            self.stop.set()
            await asyncio.get_running_loop().run_in_executor(None, self.thread.join)

    class OdometryNode(base):
        def _mount(self, sdk):
            super()._mount(sdk)
            import auki_registry as registry
            session = str(uuid.uuid4())
            owner = self._peer.peer_id
            # A fresh destination on each publisher session prevents accidental
            # reuse of an alignment after a runner restart or vendor reset.
            source = registry.frame_ros_body(owner, 'robot.base')
            target = registry.frame_ros_body(owner, 'robot.odometry.' + session)
            source.pop('hash', None)
            target.pop('hash', None)
            registry.write_frame(self._app_root, source)
            registry.write_frame(self._app_root, target)
            clock_id = 'galbot.odometry.' + session
            entry = registry.utc_clock_entry(peer_id=owner, session_id=session,
                clock_id=clock_id, unit='nanoseconds', scope='device-local',
                epoch='1970-01-01T00:00:00Z')
            clock_hash = registry.write_clock(self._app_root, entry)
            contract = dict(from_frame=source, to_frame=target, session_id=session,
                clock=dict(peer_id=owner, id=clock_id, hash=str(clock_hash)))
            publisher = sdk.OdometryPublisher.mount(self._peer, json.dumps(contract))
            from component_teleop import control_backend
            control_clock = dict(peer_id=owner, session_id=session, clock_id='galbot.control.' + session,
                type='monotonic_clock', unit='nanoseconds', monotonic=True, epoch=None, scope='device-local')
            digest = registry.write_clock(self._app_root, control_clock)
            publisher.mount_control(json.dumps(dict(peer_id=owner, id=control_clock['clock_id'], hash=str(digest))),
                control_backend(ctx))
            endpoint = OdometryEndpoint(publisher, Path(self._app_root) / 'odometry-status.json')
            self._endpoints.append(endpoint)
            from camera_capture import CameraCaptures
            from sensor_observations import CAMERAS
            mapping_camera = os.environ.get('GALBOT_MAPPING_CAMERA', 'left_arm') if os.environ.get('GALBOT_COMPONENT_MAPPING') == '1' else None
            names = CAMERAS if os.environ.get('GALBOT_SENSOR_COMPONENTS') == '1' else ([mapping_camera] if mapping_camera else [])
            endpoint.cameras = CameraCaptures(names, mapping_camera)
            ctx.camera_captures = endpoint.cameras
            if os.environ.get("GALBOT_SENSOR_COMPONENTS") == "1":
                from sensor_observations import SensorObservations
                endpoint.sensors = SensorObservations(ctx, publisher, contract, self._app_root)
            if os.environ.get('GALBOT_COMPONENT_MAPPING') == '1':
                from live_mapping import LiveMapping
                endpoint.mapping = LiveMapping(ctx, publisher, contract, dict(self._written.clock), self._app_root)
    return OdometryNode
