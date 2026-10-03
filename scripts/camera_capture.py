"""Bounded camera acquisition outside the control process's Python GIL.

Only read-only vendor APIs run here. The owning peer still publishes every frame
through its native SDK Camera Components and retains their ten-second Products.
"""
import os
import multiprocessing
import queue
import threading
import time


def _latest_put(channel, value):
    """A slow publisher skips old capture packets instead of accumulating latency."""
    try:
        channel.put_nowait(value)
    except queue.Full:
        try:
            channel.get_nowait()
        except queue.Empty:
            pass
        try:
            channel.put_nowait(value)
        except queue.Full:
            pass


def _mapping_capture(robot, sensor, optical):
    from galbot_sdk.g1 import SensorType
    import importlib.util
    from pathlib import Path
    depth_sensor = SensorType.LEFT_ARM_DEPTH_CAMERA if optical == 'left_arm_camera_color_optical_frame' else SensorType.RIGHT_ARM_DEPTH_CAMERA
    if not hasattr(_mapping_capture,'helper'):
        spec=importlib.util.spec_from_file_location('rgbd_helper',Path(__file__).with_name('capture-rgbd.py'))
        helper=importlib.util.module_from_spec(spec);spec.loader.exec_module(helper)
        # Install the cache only after calibration succeeds. A camera that is
        # still starting must be retried, not left with a half-created cache.
        info, provenance = helper.calibration(robot,sensor,depth_sensor,optical)
        _mapping_capture.info, _mapping_capture.provenance = info, provenance
        _mapping_capture.helper=helper
    return _mapping_capture.helper.capture(robot,sensor,depth_sensor,_mapping_capture.info,_mapping_capture.provenance,True,optical)


def _camera_process(sensor_name, optical, mapping, output, mapping_output, stopped):
    # spawn, never fork: inheriting the parent's DDS/native threads is unsafe.
    from galbot_sdk.g1 import GalbotRobot, SensorType, RgbOutputFormat
    parent = os.getppid()
    robot = GalbotRobot()
    sensor = getattr(SensorType, sensor_name)
    depth_sensor = SensorType.LEFT_ARM_DEPTH_CAMERA if sensor_name == "LEFT_ARM_CAMERA" else SensorType.RIGHT_ARM_DEPTH_CAMERA
    output.cancel_join_thread()
    mapping_output.cancel_join_thread()
    try:
        if not robot.init({sensor, depth_sensor} if mapping else {sensor}, mapping):
            raise RuntimeError('read-only camera initialization failed')
        last_mapping = 0.0
        while not stopped.is_set() and os.getppid() == parent:
            started = time.monotonic()
            try:
                frame = robot.get_rgb_data(sensor, RgbOutputFormat.JPEG, False)
                packet = dict(rgb=frame or None,
                              intrinsics=robot.get_camera_intrinsic(sensor) if frame else None)
                if mapping and started - last_mapping >= 1.0:
                    try:
                        _latest_put(mapping_output, dict(mapping=_mapping_capture(robot, sensor, optical)))
                    except Exception as exc:
                        _latest_put(mapping_output, dict(mapping_error=str(exc)))
                    last_mapping = started
                packet['read_duration_ms'] = (time.monotonic() - started) * 1000
                _latest_put(output, packet)
            except Exception as exc:
                _latest_put(output, dict(error=str(exc)))
            stopped.wait(max(.001, .2 - (time.monotonic() - started)))
    except Exception as exc:
        _latest_put(output, dict(error=str(exc)))
    finally:
        robot.request_shutdown()
        robot.destroy()


class CameraWorker:
    def __init__(self, name, sensor_name, optical, mapping):
        context = multiprocessing.get_context('spawn')
        self.channel = context.Queue(maxsize=1)
        self.mapping_channel = context.Queue(maxsize=1)
        self.stopped = context.Event()
        self.lock = threading.Lock()
        self.latest = {}
        self.process = context.Process(target=_camera_process,
            args=(sensor_name, optical, mapping, self.channel, self.mapping_channel, self.stopped),
            name='camera-reader-' + name, daemon=True)
        self.process.start()

    def poll(self):
        with self.lock:
            for channel in (self.channel, self.mapping_channel):
                while True:
                    try:
                        packet = channel.get_nowait()
                    except queue.Empty:
                        break
                    if 'error' in packet:
                        self.latest = packet
                    else:
                        self.latest.pop('error', None)
                        if 'mapping' in packet:self.latest.pop('mapping_error', None)
                        self.latest.update(packet)
            if not self.process.is_alive():
                raise RuntimeError('camera acquisition worker exited')
            if 'error' in self.latest:
                raise RuntimeError(self.latest['error'])
            return dict(self.latest)

    def close(self):
        self.stopped.set()
        self.process.join(3)
        if self.process.is_alive():
            self.process.terminate()
            self.process.join(2)
        self.channel.close()
        self.mapping_channel.close()


class CameraCaptures:
    def __init__(self, names, mapping_camera=None):
        from galbot_sensor_names import galbot_sensor_types, galbot_sensor_link_names
        self.workers = {name: CameraWorker(name, galbot_sensor_types[name].name,
                            galbot_sensor_link_names[name], name == mapping_camera)
                        for name in names}

    def rgb(self, name):
        packet = self.workers[name].poll()
        return packet.get('rgb'), packet.get('intrinsics')

    def mapping(self, name):
        packet=self.workers[name].poll()
        if 'mapping_error' in packet:raise RuntimeError(packet['mapping_error'])
        return packet.get('mapping')

    def close(self):
        for worker in self.workers.values():
            worker.stopped.set()
        for worker in self.workers.values():
            worker.close()
