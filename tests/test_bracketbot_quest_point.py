import sys
import unittest
from pathlib import Path
from unittest import mock
import numpy as np
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_capture import rotation_quaternion
from bracketbot_aim import aim_error, hand_axis
import bracketbot_quest_point as qp

# A toy right arm whose IK is exact: j1..j3 move the hand (1 m per turn) from
# SHOULDER, j4..j6 are its rotation vector (1 turn = 2 pi). j0 is the mast.
SHOULDER = np.array([0., -.25, .60])
LIMITS = (np.array([-3.5, -.6, -.6, -.6, -.45, -.45, -.45, 0.]),
          np.array([0., .6, .6, .6, .45, .45, .45, .2]))
# BBOS's cfg.home: hand ahead and up, fingers forward (+90 deg about Y).
HOME = np.array([0., .3, 0., .35, 0., .25, 0., 0.])
# Resting: hand low, fingers down and forward. The gripper is as found.
START = np.array([0., 0., 0., 0., 0., .4, 0., .05])
RATE = 50


def rodrigues(v):
    angle = np.linalg.norm(v)
    if angle < 1e-12:
        return np.eye(3)
    k = np.array([[0, -v[2], v[1]], [v[2], 0, -v[0]], [-v[1], v[0], 0]])/angle
    return np.eye(3)+np.sin(angle)*k+(1-np.cos(angle))*k@k


def fk(q):
    q = np.asarray(q, float)
    return SHOULDER+q[1:4], rotation_quaternion(rodrigues(2*np.pi*q[4:7]))


def solve(pos, quat, like=START):
    x, y, z, w = np.asarray(quat, float)*(1 if quat[3] >= 0 else -1)
    s = np.linalg.norm([x, y, z])
    rotvec = np.zeros(3) if s < 1e-12 else 2*np.arctan2(s, w)*np.array([x, y, z])/s
    q = np.asarray(like, float).copy()
    q[0], q[1:4], q[4:7] = 0., np.asarray(pos, float)-SHOULDER, rotvec/(2*np.pi)
    return q


def plan(target, solver=solve, **changes):
    return qp.plan_point_at(fk, solver, lambda q: None, START, HOME, np.asarray(target, float),
                            *LIMITS, 'arm_right', RATE)


class QuestPointTests(unittest.TestCase):
    def test_eases_to_home_then_points_at_the_target(self):
        target = [2., -.6, 1.2]
        samples, report = plan(target)
        self.assertLess(aim_error(fk, samples[-1], target)[1], .5)
        self.assertLess(report['aim_error_deg'], .5)
        # The first leg ends at BBOS's home, gripper kept as found.
        ready = int(round(report['ready_seconds']*RATE))
        np.testing.assert_allclose(samples[ready-1][:7], HOME[:7])
        self.assertTrue(all(q[7] == START[7] for q in samples))
        steps = np.abs(np.diff(np.array([START, *samples])[:, :7], axis=0))
        self.assertLessEqual(steps.max(), qp.MAX_JOINT_TURNS_PER_S/RATE+1e-9)
        self.assertEqual(report['mast_travel_turns'], 0.)
        # The hand ends REACH_M from home toward the target.
        self.assertAlmostEqual(np.linalg.norm(fk(samples[-1])[0]-fk(HOME)[0]), qp.REACH_M, places=6)

    def test_the_finger_axis_turns_by_the_smallest_rotation(self):
        rng = np.random.default_rng(1)
        for _ in range(20):
            quat = rng.normal(size=4); quat /= np.linalg.norm(quat)
            direction = rng.normal(size=3)
            turned = qp.turn_axis_onto(quat, direction)
            np.testing.assert_allclose(hand_axis(turned), direction/np.linalg.norm(direction), atol=1e-9)
            between = np.degrees(np.arccos(np.clip(np.dot(hand_axis(quat), direction/np.linalg.norm(direction)), -1, 1)))
            self.assertAlmostEqual(qp.angle_deg(quat, turned), between, places=6)

    def test_the_hand_path_is_eased_and_speed_capped(self):
        start, goal = fk(START), qp.pointing_pose(*fk(HOME), np.array([2., -.6, 1.2]))
        path = qp.hand_path(start, goal, RATE)
        positions = np.array([start[0], *[p for p, _ in path]])
        speeds = np.linalg.norm(np.diff(positions, axis=0), axis=1)*RATE
        self.assertLessEqual(speeds.max(), qp.MAX_SPEED_MPS*1.01)
        self.assertLess(speeds[0], .1*speeds.max())   # starts gently ...
        self.assertLess(speeds[-1], .1*speeds.max())  # ... and arrives gently
        np.testing.assert_allclose(path[-1][0], goal[0])
        self.assertLess(qp.angle_deg(path[-1][1], goal[1]), 1e-6)

    def test_refuses_what_it_cannot_do_safely(self):
        with self.assertRaisesRegex(ValueError, 'too close'):
            plan(fk(HOME)[0]+[.05, 0, 0])
        with self.assertRaisesRegex(ValueError, 'cannot reach'):
            plan([2., -.6, 1.2], solver=lambda pos, quat: None)
        # Pointing behind needs a wrist past its calibrated limit.
        with self.assertRaisesRegex(ValueError, 'limit'):
            plan([-2., -.3, .8])
        # Into the robot's own side: the hand would cross the body centre.
        with self.assertRaisesRegex(ValueError, 'envelope'):
            qp.plan_point_at(fk, solve, lambda q: None, START, HOME, np.array([1., -.6, 1.]),
                             *LIMITS, 'arm_left', RATE)

    def test_a_fast_path_is_slowed_and_a_jump_refused(self):
        _, normal = plan([2., -.6, 1.2])
        with mock.patch.object(qp, 'MAX_JOINT_TURNS_PER_S', .15):
            samples, slowed = plan([2., -.6, 1.2])
        self.assertGreater(slowed['seconds'], normal['seconds'])
        reach = samples[int(round(slowed['ready_seconds']*RATE))-1:]
        self.assertLessEqual(np.abs(np.diff(np.array(reach)[:, :7], axis=0)).max(), .15/RATE+1e-9)
        calls = []

        def flipping(pos, quat):
            # An IK branch flip: the hand stays put but a joint (here the
            # toy's free mast) snaps back and forth.
            calls.append(1)
            q = solve(pos, quat)
            q[0] -= .2 if len(calls) % 40 > 20 else 0.
            return q
        with self.assertRaisesRegex(ValueError, 'jump'):
            plan([2., -.6, 1.2], solver=flipping)


if __name__ == '__main__':
    unittest.main()
