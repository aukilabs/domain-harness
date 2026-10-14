import importlib.util
from pathlib import Path
import sys
import unittest
import numpy as np

scripts = Path(__file__).resolve().parents[1] / 'scripts'
sys.path.insert(0, str(scripts))
spec = importlib.util.spec_from_file_location('diagnostic_depth', scripts / 'diagnostic-depth.py')
depth = importlib.util.module_from_spec(spec)
spec.loader.exec_module(depth)


class DiagnosticDepthTests(unittest.TestCase):
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
