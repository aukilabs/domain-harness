"""Standalone camera initialization must recover after transient calibration failure."""
import importlib.util
import sys
import types
import unittest
from pathlib import Path
from unittest.mock import patch

spec=importlib.util.spec_from_file_location('camera_capture',Path(__file__).resolve().parents[1]/'scripts/camera_capture.py')
camera=importlib.util.module_from_spec(spec);spec.loader.exec_module(camera)

class CameraStartupTests(unittest.TestCase):
    def test_failed_calibration_retries_without_partial_cache(self):
        calls=[]
        def calibrate(*args):
            calls.append(args)
            if len(calls)==1:raise ValueError('camera not ready')
            return {'ready':True},'measured calibration'
        def load(module):
            module.calibration=calibrate
            module.capture=lambda *args: {'captured':args[3]['ready']}
        fake=types.SimpleNamespace(SensorType=types.SimpleNamespace(RIGHT_ARM_DEPTH_CAMERA='depth',LEFT_ARM_DEPTH_CAMERA='left_depth'))
        for key in ['helper','info','provenance']:
            if hasattr(camera._mapping_capture,key):delattr(camera._mapping_capture,key)
        with patch.dict(sys.modules,{'galbot_sdk.g1':fake}), patch.object(camera,'__file__',str(Path(__file__).resolve().parents[1]/'scripts/camera_capture.py')), patch('importlib.util.spec_from_file_location',return_value=types.SimpleNamespace(loader=types.SimpleNamespace(exec_module=load))), patch('importlib.util.module_from_spec',return_value=types.SimpleNamespace()):
            with self.assertRaisesRegex(ValueError,'camera not ready'):camera._mapping_capture(None,'rgb','optical')
            self.assertFalse(hasattr(camera._mapping_capture,'helper'))
            self.assertEqual(camera._mapping_capture(None,'rgb','optical'),{'captured':True})
            self.assertEqual(len(calls),2)

if __name__=='__main__':unittest.main()
