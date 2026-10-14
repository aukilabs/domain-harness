import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_aim import aim_step

SHOULDER = np.array([0., -.2, .9])


def toy_fk(q):
    """Straight 0.6 m right arm: J1 pitches it up from hanging, J2 swings it sideways."""
    pitch, swing = q[1]*2*np.pi, q[2]*2*np.pi
    direction = np.array([np.sin(pitch)*np.cos(swing), -np.sin(pitch)*np.sin(swing), -np.cos(pitch)])
    # Quaternion whose +Z is `direction` (shortest arc from +Z).
    axis = np.cross([0, 0, 1], direction); s = np.linalg.norm(axis)
    if s < 1e-9:
        quat = [0, 0, 0, 1] if direction[2] > 0 else [1, 0, 0, 0]
    else:
        angle = np.arctan2(s, direction[2]); axis /= s
        quat = [*(axis*np.sin(angle/2)), np.cos(angle/2)]
    return SHOULDER+.6*direction, np.array(quat)


class AimTests(unittest.TestCase):
    def test_repeated_bounded_steps_converge_on_a_reachable_target(self):
        lower, upper = np.full(8, -.5), np.full(8, .5)
        q, target = np.zeros(8), np.array([1.2, -.6, 1.1])
        q[1] = .05  # just off straight down
        for _ in range(400):
            q, err, _ = aim_step(toy_fk, q, target, lower, upper, max_step=.01)
        self.assertLess(err, 2)

    def test_each_step_respects_speed_and_joint_limits(self):
        lower, upper = np.full(8, -.1), np.full(8, .1)
        q = np.zeros(8); q[1] = .05
        nxt, _, _ = aim_step(toy_fk, q, np.array([1.2, -.6, 1.6]), lower, upper, max_step=.004)
        self.assertLessEqual(np.max(np.abs(nxt-q)), .004+1e-12)
        self.assertTrue(np.all(nxt >= lower) and np.all(nxt <= upper))

    def test_step_that_would_leave_the_envelope_holds_position(self):
        # Target across the body: following it would swing the right hand past centre.
        lower, upper = np.full(8, -.5), np.full(8, .5)
        q = np.zeros(8); q[1] = .2; q[2] = -.02
        for _ in range(300):
            q, _, moved = aim_step(toy_fk, q, np.array([.5, 1.5, 1.]), lower, upper, max_step=.01)
        self.assertGreaterEqual(-toy_fk(q)[0][1], .05)

    def test_target_on_top_of_the_hand_is_refused(self):
        q = np.zeros(8); q[1] = .25
        with self.assertRaisesRegex(ValueError, 'too close'):
            aim_step(toy_fk, q, toy_fk(q)[0], np.full(8, -.5), np.full(8, .5), .01)


if __name__ == '__main__': unittest.main()
