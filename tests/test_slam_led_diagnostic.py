import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('diagnostic', Path(__file__).parents[1] / 'scripts/bracketbot_slam_led_diagnostic.py')
diagnostic = importlib.util.module_from_spec(spec)
spec.loader.exec_module(diagnostic)


class StatusTests(unittest.TestCase):
    def test_pose_preserves_bbos_coordinates_and_capture_time(self):
        class Timestamp:
            def astype(self, _): return 1_000_000_000
        record = dict(timestamp=Timestamp(), pos=[1, 2, 3], quat=[0, 0, 0, 1])
        sample = diagnostic.pose_sample(record, 'tracking', 1_010_000_000, 42)
        self.assertEqual(sample['timestamp_ns'], 1_000_000_000)
        self.assertEqual(sample['pose']['translation'], [1, 2, 3])
        self.assertEqual(sample['pose']['rotation_xyzw'], [0, 0, 0, 1])
        self.assertEqual(sample['source_id'], 42)
        self.assertIsNone(diagnostic.pose_sample(record, 'lost', 1_010_000_000, 42))
        self.assertIsNone(diagnostic.pose_sample(record, 'tracking', 2_000_000_000, 42))
        self.assertIsNone(diagnostic.pose_sample(dict(record, quat=[0, 0, 0, 0]), 'tracking', 1_010_000_000, 42))
        self.assertIsNone(diagnostic.pose_sample(dict(record, pos=[float('nan'), 0, 0]), 'tracking', 1_010_000_000, 42))

    def test_restarts_back_off_and_are_bounded(self):
        delays = [diagnostic.restart_delay(n) for n in range(10)]
        self.assertEqual(delays[:6], [2, 4, 8, 16, 32, 60])
        self.assertTrue(all(d == 60 for d in delays[6:]), 'never faster than once a minute when failing')
        self.assertGreater(diagnostic.STEADY_RUN_S, delays[0], 'a run that lasted resets the backoff')

    def test_red_trumps_green(self):
        for connected in (False, True):
            for state in ('lost', 'stalled'):
                self.assertEqual(diagnostic.led_color(state, connected), [255, 0, 0])
        self.assertEqual(diagnostic.led_color('tracking', True), [0, 255, 0])
        self.assertIsNone(diagnostic.led_color('tracking', False))
        for state in ('unlocalized', 'unknown (no fresh BBOS health)'):
            self.assertIsNone(diagnostic.led_color(state, True))

    def test_native_flags_and_recovery(self):
        health = dict(vo_lost=False, stalled=False, localized=True)
        self.assertEqual(diagnostic.status(health, 10, 11), 'tracking')
        for flag in ('vo_lost', 'stalled'):
            changed = dict(health, **{flag: True})
            self.assertEqual(diagnostic.status(changed, 10, 11), 'lost' if flag == 'vo_lost' else 'stalled')
        self.assertEqual(diagnostic.status(dict(health, localized=False), 10, 11), 'unlocalized')

    def test_missing_and_stale_are_not_tracking_loss(self):
        self.assertTrue(diagnostic.status(None, 0, 10).startswith('unknown'))
        self.assertTrue(diagnostic.status(dict(vo_lost=True), 0, 4).startswith('unknown'))

    def test_pointing_alert_blinks_then_ends(self):
        self.assertEqual(diagnostic.alert_color('hot', 0), [255, 0, 0])
        self.assertEqual(diagnostic.alert_color('hot', .3), [0, 0, 0])
        self.assertEqual(diagnostic.alert_color('unreachable', 0), [255, 140, 0])
        self.assertEqual(diagnostic.alert_color('anything', .1), [255, 0, 0])
        self.assertIsNone(diagnostic.alert_color('hot', diagnostic.ALERT_SECONDS+.1))

    def test_every_alert_code_has_words_and_none_reach_a_shell(self):
        for code in ('hot', 'current', 'unreachable'):
            self.assertIn(code, diagnostic.ALERT_SPEECH)
        self.assertTrue(all(isinstance(t, str) and t for t in diagnostic.ALERT_SPEECH.values()))


if __name__ == '__main__':
    unittest.main()
