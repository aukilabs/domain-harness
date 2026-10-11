from pathlib import Path
import sys
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_drive import MAX_INPUT_AGE_MS, MAX_WHEEL_MPS, STOP, twist

PHONE = 'phone-peer'
R = 0.16375


def packet(forward=0.0, turn=0.0, *, driver=PHONE, age_ms=20, request_ms=10, received_ns=1_000_000_000, **extra):
    return dict(schema='spatial.robot-drive-input/v1', authenticated=True, allowed_driver_peer_id=PHONE,
                request_age_ms=request_ms, received_ns=received_ns,
                command=dict(driver_peer_id=driver, session='s', sequence=1, forward=forward, turn=turn, age_ms=age_ms),
                **extra)


def drive(p, now_ns=1_050_000_000, **kw):
    return twist(p, allowed_peer=PHONE, now_ns=now_ns, half_width_m=R, **kw)


class DrivePolicy(unittest.TestCase):
    def test_forward_and_turn_use_teleop_limits_at_half_speed(self):
        (v, w), reason = drive(packet(forward=1))
        self.assertIsNone(reason)
        self.assertAlmostEqual(v, 0.10)
        self.assertEqual(w, 0)
        (v, w), reason = drive(packet(turn=1))
        self.assertAlmostEqual(v, 0)
        self.assertAlmostEqual(w, 0.075 / R)  # left: right wheel faster
        (v, w), _ = drive(packet(forward=1), scale=1.0)
        self.assertAlmostEqual(v, 0.20)

    def test_wheels_never_exceed_the_limit(self):
        for forward, turn in ((1, 1), (1, -1), (-1, 1), (-1, -1)):
            (v, w), reason = drive(packet(forward=forward, turn=turn), scale=1.0)
            self.assertIsNone(reason)
            self.assertLessEqual(max(abs(v - w * R), abs(v + w * R)), MAX_WHEEL_MPS + 1e-9)

    def test_everything_else_is_a_stop(self):
        stops = {
            'centered': packet(),
            'inside deadzone': packet(forward=0.05, turn=-0.05),
            'other phone': packet(forward=1, driver='someone-else'),
            'home unreachable': dict(packet(forward=1), authenticated=False),
            'no command': dict(packet(forward=1), command=None),
            'empty file': {},
            'wrong schema': dict(packet(forward=1), schema='other'),
            'out of range': packet(forward=1.5),
            'not a number': packet(forward=float('nan')),
            'stale at home': packet(forward=1, age_ms=MAX_INPUT_AGE_MS),
            'slow request': packet(forward=1, request_ms=MAX_INPUT_AGE_MS),
            'future cache': packet(forward=1, received_ns=2_000_000_000),
        }
        for name, p in stops.items():
            with self.subTest(name):
                value, reason = drive(p)
                self.assertEqual(value, STOP)
                self.assertTrue(reason)
        value, reason = drive(packet(forward=1), now_ns=1_000_000_000 + (MAX_INPUT_AGE_MS + 1) * 1_000_000)
        self.assertEqual(value, STOP, 'a runner that stopped refreshing the cache stops the robot')
        self.assertEqual(twist(packet(forward=1), allowed_peer='', now_ns=1_050_000_000, half_width_m=R)[0], STOP)
        mismatched = dict(packet(forward=1), allowed_driver_peer_id='someone-else')
        self.assertEqual(drive(mismatched)[0], STOP, 'runner and controller must agree on the allowed phone')

    def test_age_sums_every_hop(self):
        self.assertIsNone(drive(packet(forward=1, age_ms=150, request_ms=100))[1])  # 150+100+50 = 300
        self.assertTrue(drive(packet(forward=1, age_ms=250, request_ms=120))[1])    # 250+120+50 = 420


if __name__ == '__main__':
    unittest.main()
