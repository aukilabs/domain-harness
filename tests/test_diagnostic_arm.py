import importlib.util
from pathlib import Path
import sys
import unittest
import uuid
import numpy as np

scripts = Path(__file__).resolve().parents[1] / 'scripts'
sys.path.insert(0, str(scripts))
sys.path.insert(0, str(Path(__file__).resolve().parent))
spec = importlib.util.spec_from_file_location('diagnostic_arm', scripts / 'diagnostic-arm.py')
arm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(arm)
from bracketbot_aim import AXLE_HEIGHT_M, aim_error
from bracketbot_point import HOME
from test_bracketbot_point import LIMITS, fk


def request(**changes):
    value = dict(request_id=str(uuid.uuid4()), arm='right', target_arm_base=[2., -.6, 1.2])
    value.update(changes)
    return __import__('json').dumps(value)


STATE = np.dtype([('pos', 'f8', 8), ('vel', 'f8', 8), ('cal_min', 'f8', 8), ('cal_max', 'f8', 8)])


def state_record(pos, vel=0., limits=LIMITS):
    """An arm_*.state record as newer BBOS publishes it, with its calibration."""
    record = np.zeros((), dtype=STATE)
    record['pos'], record['vel'] = pos, np.full(8, vel)
    # cal_min is the named negative extreme, not always the smaller number.
    record['cal_min'], record['cal_max'] = limits[1], limits[0]
    return record


class FakeArms:
    """The adapter's Arms with toy kinematics and a resting right arm."""

    def __init__(self, pos=None, vel=0.):
        self.arms = {'arm_right': dict(fk=fk, cfg=None)}
        self.state = state_record(np.array(HOME['arm_right'] if pos is None else pos), vel)

    def healthy(self, name, arm_max_age=.25):
        return self.state


def rotate(xyzw, v):
    x, y, z, w = xyzw
    q, v = np.array([x, y, z]), np.asarray(v, float)
    return v + 2*np.cross(q, np.cross(q, v) + w*v)


class DiagnosticArmTests(unittest.TestCase):
    def test_arm_base_is_declared_forward_left_up_at_the_axle(self):
        mount = arm.ARM_BASE_IN_BASE
        # arm_base +X (forward) is BBOS base +Y; +Y (left) is base -X; +Z stays up.
        np.testing.assert_allclose(rotate(mount['rotation_xyzw'], [1, 0, 0]), [0, 1, 0], atol=1e-12)
        np.testing.assert_allclose(rotate(mount['rotation_xyzw'], [0, 1, 0]), [-1, 0, 0], atol=1e-12)
        np.testing.assert_allclose(mount['translation'], [0, 0, AXLE_HEIGHT_M])

    def test_requests_are_strict(self):
        request_id, name, target = arm.parse_request(request())
        self.assertEqual((name, target.tolist()), ('arm_right', [2., -.6, 1.2]))
        self.assertEqual(str(uuid.UUID(request_id)), request_id)
        for bad in (request(arm='both'), request(request_id='x'), request(target_arm_base=[1, 2]),
                    request(target_arm_base=[1, float('nan'), 0]), request(extra=1)):
            with self.assertRaises(ValueError):
                arm.parse_request(bad)

    def test_plan_from_rest_aims_and_keeps_the_gripper_as_found(self):
        rest = HOME['arm_right'].copy(); rest[7] = .05
        start, home, forward, aimed, report = arm.plan(FakeArms(rest), 'arm_right', np.array([2., -.6, 1.2]))
        self.assertLess(aim_error(fk, aimed, [2., -.6, 1.2])[1], 1.)
        self.assertTrue(start[7] == home[7] == forward[7] == aimed[7] == .05)
        self.assertLess(report['aim_error_deg'], 1.)

    def test_limits_come_from_live_state_or_the_older_file(self):
        lower, upper = arm.measured_limits(None, 'arm_right', state_record(HOME['arm_right']))
        np.testing.assert_array_equal(lower, LIMITS[0])
        np.testing.assert_array_equal(upper, LIMITS[1])
        flat = state_record(HOME['arm_right'], limits=(np.zeros(8), np.zeros(8)))
        with self.assertRaisesRegex(RuntimeError, 'no calibrated travel'):
            arm.measured_limits(None, 'arm_right', flat)
        # Older BBOS: no cal fields in the state, ranges.calibration.json beside the daemon.
        import json, tempfile, types
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root/'arm_right/urdf').mkdir(parents=True)
            cfg = types.SimpleNamespace(urdf_path=str(root/'arm_right/urdf/arm.urdf'))
            with self.assertRaisesRegex(RuntimeError, 'not range-calibrated'):
                arm.measured_limits(cfg, 'arm_right', dict(pos=HOME['arm_right']))
            (root/'arm_right/ranges.calibration.json').write_text(json.dumps(
                dict(cal_min=LIMITS[1].tolist(), cal_max=LIMITS[0].tolist())))
            lower, _ = arm.measured_limits(cfg, 'arm_right', dict(pos=HOME['arm_right']))
            np.testing.assert_array_equal(lower, LIMITS[0])

    def test_plan_refuses_a_moving_arm_and_an_unreachable_target(self):
        with self.assertRaisesRegex(ValueError, 'moving'):
            arm.plan(FakeArms(vel=.1), 'arm_right', np.array([2., -.6, 1.2]))
        with self.assertRaises(ValueError):
            arm.plan(FakeArms(), 'arm_right', np.array([.6, 1.5, .73]))


if __name__ == '__main__':
    unittest.main()
