import math
import copy
import importlib.util
import threading
from unittest.mock import MagicMock, patch
from pathlib import Path
import sys
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_volume_speech import Entries, contains, transform_point, inputs
from test_bracketbot_pointing import fixtures

spec = importlib.util.spec_from_file_location('volume_controller',
    Path(__file__).resolve().parents[1]/'scripts/speak-bracketbot-volumes.py')
controller = importlib.util.module_from_spec(spec)
spec.loader.exec_module(controller)


def box(identity='one', description='Welcome'):
    return dict(id=identity, center=[0,0,0], size=[2,2,4], yaw=0, description=description)


class VolumeSpeechTests(unittest.TestCase):
    def test_text_is_stdin_and_playback_uses_bbos(self):
        process = MagicMock()
        process.poll.return_value = 0
        process.returncode = 0
        text = '--evil; $(touch /tmp/nope)'
        with patch.object(controller.subprocess, 'Popen', return_value=process) as popen:
            controller.speak(text, '/usr/bin/espeak-ng', threading.Event())
        self.assertEqual(popen.call_count, 2)
        synthesis, playback = popen.call_args_list
        self.assertNotIn(text, synthesis.args[0])
        process.stdin.write.assert_called_once_with(text.encode())
        self.assertIn('speaker', playback.args[0])
        self.assertTrue(any(p.endswith('play-bracketbot-speech.py') for p in playback.args[0]))
        self.assertNotIn('shell', synthesis.kwargs)

    def test_rigid_transform(self):
        pose = dict(rotation_wxyz=[math.sqrt(.5),0,0,math.sqrt(.5)], translation=[3,4,5])
        result = transform_point(pose, [1,0,0])
        for a,b in zip(result, [3,5,5]): self.assertAlmostEqual(a,b)
        with self.assertRaises(ValueError): transform_point(dict(pose, rotation_wxyz=[2,0,0,0]), [0,0,0])

    def test_yaw_and_height(self):
        volume = dict(box(), yaw=math.pi/2)
        self.assertTrue(contains(volume, [1.5,0,0]))
        self.assertFalse(contains(volume, [0,0,1.5]))
        self.assertFalse(contains(volume, [0,1.1,0]))

    def test_dwell_hysteresis_reentry(self):
        entries = Entries(); volumes = [box()]
        self.assertEqual(entries.update([0,0,0], volumes, 0, 'home'), [])
        self.assertEqual(entries.update([0,0,0], volumes, .6, 'home'), [('one','Welcome')])
        self.assertEqual(entries.update([1.03,0,0], volumes, 1, 'home'), [])
        self.assertIn('one', entries.inside)
        entries.update([1.1,0,0], volumes, 2, 'home')
        self.assertNotIn('one', entries.inside)
        entries.update([0,0,0], volumes, 3, 'home')
        self.assertEqual(entries.update([0,0,0], volumes, 4, 'home'), [('one','Welcome')])

    def test_tracking_loss_is_not_exit(self):
        entries=Entries(); volumes=[box()]
        entries.update([0,0,0],volumes,0,'home'); entries.update([0,0,0],volumes,1,'home')
        entries.update(None,None,2,'home')
        self.assertEqual(entries.update([0,0,0],volumes,3,'home'), [])

    def test_overlaps_blank_deleted_scope(self):
        entries=Entries(); volumes=[box('b'),box('a'),box('blank','  ')]
        entries.update([0,0,0],volumes,0,'home')
        self.assertEqual(entries.update([0,0,0],volumes,1,'home'), [('a','Welcome'),('b','Welcome')])
        entries.update(None,[],2,'home'); self.assertFalse(entries.inside)
        entries.update([0,0,0],volumes,3,'another')
        self.assertEqual(len(entries.update([0,0,0],volumes,4,'another')),2)

    def test_invalid_inputs_are_silent(self):
        self.assertIsNone(inputs({}, {}, {}, now_ns=0, capture_now_ns=0, scene_age_ms=0))

    def test_authenticated_base_position_and_freshness(self):
        scene, _, _ = fixtures()
        mapping = scene['registries']['local_mapping']
        mapping['status'] = 'mapping'
        mapping['robot_pose']['capture_timestamp_ns'] = 1000000000
        profile = dict(peer_id='home-owner', frame='home', product_id='scene')
        packet = dict(schema='spatial.robot-volume-input/v1', authenticated=True,
                      robot_peer_id='robot', home_peer_id='home-owner', home_frame='home',
                      received_ns=1000000000,
                      volumes=dict(schema='spatial-harness.semantic-volumes/v1', frame='home', items=[box()]))
        def evaluate(s=scene, p=packet, **ages):
            values=dict(now_ns=1000000000, capture_now_ns=1000000000, scene_age_ms=0)
            values.update(ages)
            return inputs(s,p,profile,**values)
        self.assertEqual(evaluate()[0], (11,0,0))
        for ages in [dict(scene_age_ms=501),dict(capture_now_ns=1801000000),
                     dict(now_ns=16001000000),dict(now_ns=999999999)]:
            self.assertIsNone(evaluate(**ages))
        for key, value in [('authenticated',False),('home_peer_id','wrong'),
                           ('home_frame','wrong'),('robot_peer_id','wrong')]:
            self.assertIsNone(evaluate(p=dict(packet, **{key:value})))
        for validity in ['stale','unavailable']:
            wrong=copy.deepcopy(scene)
            wrong['registries']['frame_transform']['validity']=validity
            self.assertIsNone(evaluate(s=wrong))
        wrong=copy.deepcopy(scene)
        wrong['registries']['local_mapping']['robot_pose']['from_frame']['frame_id']='camera'
        self.assertIsNone(evaluate(s=wrong))


if __name__ == '__main__': unittest.main()
