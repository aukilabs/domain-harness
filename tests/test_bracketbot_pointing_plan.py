import sys
from pathlib import Path
import tempfile
import unittest
import numpy as np
from scipy.spatial.transform import Rotation

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_pointing_plan import base_from_arm, pointing_rotation, candidate


class IK:
    def reset(self, q):
        pass

    def fk(self, q):
        return np.array([0., 0., 1.]), Rotation.from_rotvec(q[1:4]).as_quat()

    def solve(self, position, quat):
        return [0., *Rotation.from_quat(quat).as_rotvec(), 0., 0., 0.]


class Config:
    ik = IK()
    q2urdf = staticmethod(lambda q: q.copy())
    urdf2q = staticmethod(lambda q: q.copy())


class PlannerTests(unittest.TestCase):
    def test_ray_alignment_and_roll_preservation(self):
        original = Rotation.from_euler('z', 40, degrees=True)
        result = pointing_rotation(original, [.2, 0, 1])
        np.testing.assert_allclose(result.apply([0, 0, 1]), np.array([.2, 0, 1])/np.linalg.norm([.2, 0, 1]))
        np.testing.assert_allclose(pointing_rotation(original, [0, 0, 1]).as_matrix(), original.as_matrix())
        for direction in ([0, 0, 0], [0, 0, -1], [float('nan'), 0, 1]):
            with self.assertRaises(ValueError): pointing_rotation(original, direction)

    def test_candidate_keeps_mast_gripper_and_is_never_executable(self):
        start = np.zeros(8); start[0] = .01; start[7] = .05
        result = candidate(Config(), start, [.15, 0, 2], np.full(8, -.5), np.full(8, .5))
        self.assertFalse(result['actuation_enabled'])
        self.assertEqual(result['collision_status'], 'not_checked')
        for q in result['trajectory_motor_turns']:
            self.assertEqual(q[0], start[0]); self.assertEqual(q[7], start[7])
        self.assertEqual(len(result['trajectory_motor_turns']), 41)

    def test_bad_ik_and_discontinuous_solutions_rejected(self):
        args = (np.zeros(8), [.15, 0, 2], np.full(8, -.5), np.full(8, .5))
        for solution in ([float('nan')]*7, [0]*6, [0, .3, 0, 0, 0, 0, 0], [0]*7):
            cfg = Config(); cfg.ik = IK(); cfg.ik.solve = lambda p, q, value=solution: value
            with self.assertRaises(ValueError): candidate(cfg, *args)

    def test_joint_limits_and_invalid_start_rejected(self):
        with self.assertRaises(ValueError):
            candidate(Config(), np.ones(8), [.15, 0, 2], np.full(8, -.5), np.full(8, .5))
        with self.assertRaises(ValueError):
            candidate(Config(), np.zeros(8), [.15, 0, 2], np.full(8, -.01), np.full(8, .01))

    def test_urdf_fixed_transform_not_assumed_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/'robot.urdf'
            xml = '<robot><joint name="base" type="fixed"><parent link="root"/><child link="arm_base"/><origin xyz="1 0 0" rpy="0 0 0"/></joint></robot>'
            path.write_text(xml)
            transform = base_from_arm(path)
            np.testing.assert_allclose((transform @ [0, 0, 0, 1])[:3], [0, 1, 0], atol=1e-12)
            path.write_text(xml.replace('type="fixed"', 'type="revolute"'))
            with self.assertRaises(ValueError): base_from_arm(path)


if __name__ == '__main__':
    unittest.main()
