import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('diagnostic', Path(__file__).parents[1] / 'scripts/bracketbot_slam_led_diagnostic.py')
diagnostic = importlib.util.module_from_spec(spec)
spec.loader.exec_module(diagnostic)


class StatusTests(unittest.TestCase):
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


if __name__ == '__main__':
    unittest.main()
