import importlib.util
from pathlib import Path
import sys
import struct
import io
import types
from unittest.mock import patch
import unittest
import numpy as np

scripts = Path(__file__).resolve().parents[1] / 'scripts'
sys.path.insert(0, str(scripts))
spec = importlib.util.spec_from_file_location('diagnostic_depth', scripts / 'diagnostic-depth.py')
depth = importlib.util.module_from_spec(spec)
spec.loader.exec_module(depth)


class DiagnosticDepthTests(unittest.TestCase):
    def test_idle_and_disabled_helper_does_not_read_or_encode_points(self):
        state = {'ticks': 0, 'reads': 0}
        stamp = 1_750_000_000_123_456_789
        record = dict(timestamp=np.datetime64(stamp, 'ns'), num_points=1,
                      points=np.array([[0., 0., 2.]]), colors=np.array([[1, 2, 3]], dtype=np.uint8))
        class Reader:
            _writer_pid = 123
            data = record
            def __init__(self, *args, **kwargs): pass
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def ready(self): state['reads'] += 1; return True
        fake = types.SimpleNamespace(Reader=Reader, Config=lambda _: types.SimpleNamespace(
            imu_pitch=0, camera_to_base_3x4=np.eye(3, 4)))
        sink = io.TextIOWrapper(io.BytesIO(), encoding='utf-8')
        def sleep(_): state['ticks'] += 1
        # Three ticks: idle, subscribe, disable. Only the subscribed tick reads BBOS.
        with patch.dict(sys.modules, {'bbos': fake}), patch.object(depth.sys, 'stdout', sink), \
             patch.object(depth.os, 'getppid', lambda: 123 if state['ticks'] < 3 else 456), \
             patch.object(depth.select, 'select', lambda *a: ([sys.stdin] if state['ticks'] else [], [], [])), \
             patch.object(depth.os, 'read', lambda *a: b'1\n' if state['ticks'] == 1 else b'0\n'), \
             patch.object(depth.time, 'time_ns', lambda: stamp), patch.object(depth.time, 'monotonic_ns', lambda: 1), \
             patch.object(depth.time, 'sleep', sleep):
            depth.main()
        self.assertEqual(state['reads'], 1)
        sink.flush()
        config, packet = sink.buffer.getvalue().split(b'\n', 1)
        self.assertIn(b'sensor_to_base', config)
        self.assertEqual(struct.unpack('<QIB', packet[:13]), (stamp, 1, 1))
        self.assertEqual(len(packet), 28)

    def test_binary_ipc_preserves_capture_ns_colors_and_f32_coordinates(self):
        stamp = 1_750_000_000_123_456_789
        sample = dict(timestamp_ns=stamp, points=[[.1, -.2, 2.], [.3, .4, 3.]],
                      colors=[[255, 0, 11], [1, 20, 30]])
        packet = depth.encode_capture(sample)
        self.assertEqual(struct.unpack('<QIB', packet[:13]), (stamp, 2, 1))
        self.assertEqual(len(packet), 13 + 2 * 15)
        np.testing.assert_array_equal(np.frombuffer(packet, dtype='<f4', count=6, offset=13).reshape(-1, 3),
                                      np.array(sample['points'], dtype=np.float32))
        self.assertEqual(list(packet[13 + 24:]), [255, 0, 11, 1, 20, 30])
        sample['points'][0][0] = float('nan')
        with self.assertRaises(ValueError): depth.encode_capture(sample)

    def test_sensor_origin_colors_and_capture_time_are_preserved(self):
        transform = np.array([[0, 0, 1, .2], [-1, 0, 0, .1], [0, -1, 0, .3]])
        optical = np.array([[0., 0., 2.], [.5, .2, 3.]])
        record = dict(timestamp=np.datetime64(123456789, 'ns'), num_points=2,
                      points=optical @ transform[:, :3].T + transform[:, 3],
                      colors=np.array([[255, 0, 0], [0, 255, 0]], dtype=np.uint8))
        result = depth.capture(record, transform)
        np.testing.assert_allclose(result['points'], optical)
        self.assertEqual(result['timestamp_ns'], 123456789)
        self.assertEqual(result['colors'], record['colors'].tolist())

    def test_bounds_and_no_invented_returns(self):
        points = np.tile([0., 0., 2.], (5000, 1))
        points[0] = [np.nan, 0, 1]
        points[1] = [0, 0, -1]
        record = dict(timestamp=np.datetime64(1000000, 'ns'), num_points=len(points),
                      points=points, colors=np.zeros((len(points), 3), dtype=np.uint8))
        result = depth.capture(record, np.eye(3, 4))
        self.assertLessEqual(len(result['points']), 2048)
        self.assertTrue(all(p == [0., 0., 2.] for p in result['points']))
        record['num_points'] = len(points) + 1
        with self.assertRaises(ValueError):
            depth.capture(record, np.eye(3, 4))


if __name__ == '__main__':
    unittest.main()
