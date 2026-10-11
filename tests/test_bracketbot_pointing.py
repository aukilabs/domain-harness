import copy
import math
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_pointing import select_target


def fixtures():
    clock = {'peer_id': 'robot', 'id': 'capture', 'hash': 'session-hash'}
    scene = {'authority_peer_id': 'robot', 'frame': 'bracket', 'registries': {
        'frame_transform_product': {'peer_id': 'robot', 'manifest_hash': 'hash',
                                    'product_id': 'robot-to-scene-transform'},
        'frame_transform': {'schema': 'spatial.frame-transform/v1', 'validity': 'valid',
            'authority_peer_id': 'robot', 'source_frame': {'id': 'bracket'},
            'target_frame': {'id': 'home'},
            'transform': {'from_frame_id': 'bracket', 'to_frame_id': 'home',
                          'translation': [10, 0, 0], 'rotation_wxyz': [1, 0, 0, 0]},
            'evidence': {'portal': 'portal', 'clock': clock, 'capture_timestamp_ns': 123}},
        'local_mapping': {'robot_pose': {'frame': 'bracket', 'clock': clock,
            'from_frame': {'frame_id': 'robot.base'},
            'pose': {'translation': [1, 0, 0], 'rotation_xyzw': [0, 0, 0, 1]}},
            'observed_portals': {'portal': {'clock': clock, 'capture_timestamp_ns': 123}}}}}
    return scene, {'frame': 'home', 'poses': []}, {'left': [-.2, 0, .7], 'right': [.2, 0, .7]}


def phone(peer, point, age=0):
    return {'peer_id': peer, 'clock': {'peer_id': peer, 'session_id': peer+'-session'},
            'received_age_ms': age, 'capture_timestamp_ns': 999999999999999999,
            'pose': {'from_frame_id': 'phone-camera:'+peer+'-session', 'to_frame_id': 'home',
                     'translation': point}}


def select(scene, presence, hands, **ages):
    defaults = dict(scene_age_ms=0, presence_age_ms=0, arm_age_ms=0, robot_pose_age_ms=0,
                    expected_home_frame='home')
    defaults.update(ages)
    return select_target(scene, presence, hands, **defaults)


class PointingTests(unittest.TestCase):
    def test_nearest_phone_then_nearest_measured_hand(self):
        s, p, h = fixtures()
        p['poses'] = [phone('far', [14, 3, 1]), phone('near', [10.5, 1, .7])]
        original = copy.deepcopy((s, p, h))
        result = select(s, p, h)
        self.assertEqual(result['phone_peer_id'], 'near')
        self.assertEqual(result['arm'], 'left')
        self.assertEqual(result['phone_position_m'], [-.5, 1, .7])
        self.assertAlmostEqual(math.hypot(*result['direction']), 1)
        self.assertFalse(result['actuation_enabled'])
        self.assertEqual((s, p, h), original)

    def test_inverse_rotation_and_translation_applied_once(self):
        s, p, h = fixtures()
        s['registries']['frame_transform']['transform']['rotation_wxyz'] = [2**-.5, 0, 0, 2**-.5]
        p['poses'] = [phone('phone', [8, 2, 1])]
        result = select(s, p, h)
        for actual, expected in zip(result['phone_position_m'], [1, 2, 1]):
            self.assertAlmostEqual(actual, expected)
        self.assertEqual(result['arm'], 'right')

    def test_selection_recomputed_and_ties_are_deterministic(self):
        s, p, h = fixtures()
        p['poses'] = [phone('b', [11, 2, 1]), phone('a', [11, 2, 1])]
        self.assertEqual(select(s, p, h)['phone_peer_id'], 'a')
        self.assertEqual(select(s, p, h)['arm'], 'left')
        p['poses'][1]['pose']['translation'] = [20, 0, 1]
        self.assertEqual(select(s, p, h)['phone_peer_id'], 'b')
        p['poses'] = []
        self.assertEqual(select(s, p, h)['status'], 'blocked')

    def test_stale_inputs_fail_closed(self):
        s, p, h = fixtures(); p['poses'] = [phone('phone', [12, 2, 1])]
        for field in ('scene_age_ms', 'presence_age_ms', 'arm_age_ms', 'robot_pose_age_ms'):
            for age in (1000, -1, float('nan')):
                self.assertEqual(select(s, p, h, **{field: age})['status'], 'blocked')
        p['poses'][0]['received_age_ms'] = 400
        self.assertEqual(select(s, p, h, presence_age_ms=200)['status'], 'blocked')

    def test_wrong_owner_frame_session_and_nonfinite_phone_ignored(self):
        s, p, h = fixtures(); valid = phone('phone', [12, 2, 1])
        for section, key, value in [('clock', 'peer_id', 'other'),
                                    ('pose', 'to_frame_id', 'other-scene'),
                                    ('pose', 'from_frame_id', 'old-session'),
                                    ('pose', 'translation', [float('nan'), 0, 0])]:
            bad = copy.deepcopy(valid); bad[section][key] = value; p['poses'] = [bad]
            self.assertEqual(select(s, p, h)['status'], 'blocked')

    def test_unlocalized_and_mismatched_robot_rejected(self):
        scene, p, h = fixtures(); p['poses'] = [phone('phone', [12, 2, 1])]
        for field, value in [('validity', 'stale'), ('authority_peer_id', 'other'),
                              ('source_frame', {'id': 'old-session'}),
                              ('target_frame', {'id': 'other-home'})]:
            s = copy.deepcopy(scene); s['registries']['frame_transform'][field] = value
            self.assertEqual(select(s, p, h)['status'], 'blocked')
        s = copy.deepcopy(scene); s['registries']['local_mapping']['robot_pose'] = None
        self.assertEqual(select(s, p, h)['status'], 'blocked')
        self.assertEqual(select(scene, p, {'left': h['left']})['status'], 'blocked')


if __name__ == '__main__':
    unittest.main()
