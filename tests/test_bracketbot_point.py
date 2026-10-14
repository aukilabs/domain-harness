import sys
import unittest
from pathlib import Path
import numpy as np
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_capture import rotation_quaternion
from bracketbot_aim import aim_error
from bracketbot_point import HOME, WHEEL_RADIUS, axis_error_deg, plan_point, plan_point_at

SHOULDER, LENGTH, MAST_M_PER_TURN = np.array([-.02, -.10, .73]), .50, .25
WRIST_OFFSET = np.radians(4.)  # the hanging fingers are not quite along the arm


def rx(a): return np.array([[1, 0, 0], [0, np.cos(a), -np.sin(a)], [0, np.sin(a), np.cos(a)]])
def ry(a): return np.array([[np.cos(a), 0, np.sin(a)], [0, 1, 0], [-np.sin(a), 0, np.cos(a)]])


def make_fk(side='arm_right'):
    # j0 mast, j1 sideways swing, j2 forward pitch, j5/j6 wrist roll/pitch.
    # Hanging arm and fingers point down. The left arm mirrors the right:
    # its shoulder is on +Y and its mast and pitch motors turn the other way.
    sign = 1 if side == 'arm_right' else -1
    home = HOME[side]

    def fk(q):
        turn = 2*np.pi*np.asarray(q, float)*sign
        shoulder = SHOULDER*[1, sign, 1]+[0, 0, MAST_M_PER_TURN*sign*(q[0]-home[0])]
        arm = rx(sign*turn[1])@ry(turn[2])
        hand = arm@ry(turn[6]+WRIST_OFFSET)@rx(sign*turn[5])@np.diag([1., -1., -1.])
        return shoulder+LENGTH*(arm@[0, 0, -1]), rotation_quaternion(hand)
    return fk


fk = make_fk()


LIMITS = np.array([-3.5, -.333, -.333, -.333, -.333, -.333, -.333, 0.]), np.array([0., .333, .333, .333, .333, .333, .333, .2])


class PointTests(unittest.TestCase):
    def test_right_arm_points_straight_ahead_at_shoulder_height(self):
        home = HOME['arm_right']
        target, plan = plan_point(fk, home, *LIMITS)
        self.assertEqual(plan['shoulder_joint'], 2)
        self.assertLess(axis_error_deg(fk, target), .1)
        hand = fk(target)[0]
        np.testing.assert_allclose(hand, SHOULDER+[LENGTH, 0, 0], atol=5e-3)
        # Only the shoulder pitch and wrist move: mast, elbow and gripper stay.
        np.testing.assert_array_equal(target[[0, 1, 3, 4, 7]], home[[0, 1, 3, 4, 7]])
        self.assertAlmostEqual(target[2], -.25, delta=.02)

    def test_left_arm_mirrors_the_right(self):
        fk_left = make_fk('arm_left')
        lo, hi = LIMITS
        left_limits = -hi, -lo
        target, plan = plan_point(fk_left, HOME['arm_left'], *left_limits, side='arm_left')
        self.assertLess(axis_error_deg(fk_left, target), .1)
        np.testing.assert_allclose(fk_left(target)[0], SHOULDER*[1, -1, 1]+[LENGTH, 0, 0], atol=5e-3)
        self.assertAlmostEqual(target[2], .25, delta=.02)
        np.testing.assert_array_equal(target[[0, 1, 3, 4, 7]], HOME['arm_left'][[0, 1, 3, 4, 7]])
        right, _ = plan_point(fk, HOME['arm_right'], *LIMITS)
        np.testing.assert_allclose(plan['hand_floor_m'], np.array(fk(right)[0]+[0, 0, WHEEL_RADIUS])*[1, -1, 1], atol=5e-3)
        # The right-arm path envelope rejects a left arm planned as the right.
        with self.assertRaises(ValueError):
            plan_point(fk_left, HOME['arm_left'], *left_limits, side='arm_right')

    def test_height_moves_only_the_mast(self):
        home = HOME['arm_right']
        level, _ = plan_point(fk, home, *LIMITS)
        raised, plan = plan_point(fk, home, *LIMITS, height_m=1.10)
        self.assertAlmostEqual(fk(raised)[0][2]+WHEEL_RADIUS, 1.10, places=4)
        self.assertAlmostEqual(plan['hand_floor_m'][2], 1.10, places=4)
        np.testing.assert_allclose(raised[1:], level[1:])
        self.assertNotEqual(raised[0], level[0])

    def test_limits_and_non_home_start_are_rejected(self):
        lo, hi = LIMITS
        with self.assertRaises(ValueError):
            plan_point(fk, HOME['arm_right'], np.where(np.arange(8) == 2, -.05, lo), hi)
        with self.assertRaises(ValueError):
            plan_point(fk, HOME['arm_right'], *LIMITS, height_m=3.)
        bent = HOME['arm_right'].copy(); bent[3] = .2
        with self.assertRaises(ValueError):
            plan_point(fk, bent, *LIMITS)

    def test_point_at_aims_the_straight_arm_through_the_target(self):
        for target in ([2., -.6, 1.2], [1.5, -.1, .4], [3., -.2, .73]):
            forward, aimed, plan = plan_point_at(fk, HOME['arm_right'], *LIMITS, 'arm_right', target)
            self.assertLess(aim_error(fk, aimed, target)[1], 1., target)
            self.assertLess(plan['aim_error_deg'], 1.)
            # The elbow, mast and gripper stay as the straight forward pose has them.
            np.testing.assert_array_equal(aimed[[0, 3, 7]], forward[[0, 3, 7]])
            self.assertTrue(np.all(aimed >= LIMITS[0]) and np.all(aimed <= LIMITS[1]))

    def test_point_at_refuses_targets_it_cannot_line_up_with(self):
        # Across the body: the right hand would have to swing past centre.
        with self.assertRaisesRegex(ValueError, 'cannot line up|clear envelope'):
            plan_point_at(fk, HOME['arm_right'], *LIMITS, 'arm_right', [.6, 1.5, .73])
        # On top of the hand.
        with self.assertRaises(ValueError):
            plan_point_at(fk, HOME['arm_right'], *LIMITS, 'arm_right', SHOULDER+[LENGTH, 0, 0])
        with self.assertRaises(ValueError):
            plan_point_at(fk, HOME['arm_right'], *LIMITS, 'arm_right', [np.nan, 0, 1])


if __name__ == '__main__': unittest.main()
