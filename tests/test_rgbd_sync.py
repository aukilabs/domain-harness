"""Timestamp alignment stays exact even though the vendor selects nearest frames."""
import importlib.util
from pathlib import Path
from types import SimpleNamespace as Obj
import unittest
from unittest.mock import Mock
spec=importlib.util.spec_from_file_location('rgbd',Path(__file__).resolve().parents[1]/'scripts/capture-rgbd.py')
rgbd=importlib.util.module_from_spec(spec);spec.loader.exec_module(rgbd)

class SynchronizationTests(unittest.TestCase):
    def test_rgb_anchor_selects_available_matching_depth(self):
        rgb=Obj(header=Obj(timestamp_ns=100))
        old=Obj(header=Obj(timestamp_ns=90))
        depth=Obj(header=Obj(timestamp_ns=100))
        def sample(sensors, joints):
            return Obj(rgb_data_map={'rgb':rgb if sensors[0]=='rgb' else old},depth_data_map={'depth':depth})
        robot=Obj(get_synced_observation=Mock(side_effect=sample))
        self.assertEqual(rgbd.synchronized_pair(robot,'rgb','depth'),(rgb,depth))
        robot.get_synced_observation.assert_called_once_with(['rgb','depth'],True)

    def test_nearest_frame_is_rejected_if_timestamps_differ(self):
        rgb=Obj(header=Obj(timestamp_ns=100));depth=Obj(header=Obj(timestamp_ns=101))
        robot=Obj(get_synced_observation=lambda *_:Obj(rgb_data_map={'rgb':rgb},depth_data_map={'depth':depth}))
        with self.assertRaisesRegex(ValueError,'timestamps do not match exactly'):
            rgbd.synchronized_pair(robot,'rgb','depth')

    def test_missing_observation_is_reported(self):
        with self.assertRaisesRegex(ValueError,'observation unavailable'):
            rgbd.synchronized_pair(Obj(get_synced_observation=lambda *_:None),'rgb','depth')

if __name__=='__main__':unittest.main()
