"""Generic sensor poses stay independent of images, calibration and AR."""
import importlib.util
import json
import tempfile
import threading
import types
import unittest
from pathlib import Path
from unittest.mock import patch
spec = importlib.util.spec_from_file_location('sensors', Path(__file__).resolve().parents[1] / 'scripts/sensor_observations.py')
sensors = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sensors)

class SensorPoses(unittest.TestCase):
    def capture(self, name='head_left', stamp=123):
        camera = object.__new__(sensors.SensorObservations)
        camera.lock = threading.Lock()
        camera.states, camera.counts, camera.specs, camera.last = {}, {}, {}, {}
        camera.failed = set()
        camera.owner = 'robot'
        camera.contract = dict(clock=dict(peer_id='robot', id='capture', hash='hash'), session_id='session')
        camera.read_clock = camera.contract['clock']
        def frame(owner, name):
            return dict(peer_id=owner, frame_id=name, handedness='right', units='meters')
        camera.registry = types.SimpleNamespace(frame_ros_optical=frame, frame_ros_body=frame, write_frame=lambda *a:None)
        calibration = {'width':1280,'height':992,'K':[587.,0.,648.,0.,587.,520.,0.,0.,1.], 'D':[0.,0.,0.,0.],'distortion_model':'kannala_brandt','header':{'frame_id':name+'_optical'}}
        image = {'width':1280,'height':992,'format':'jpeg','data':b'jpeg','header':{'timestamp_ns':123,'frame_id':''}}
        camera.cameras = types.SimpleNamespace(rgb=lambda n:(image,calibration))
        calls, specs, events = [], {}, {}
        camera.robot = types.SimpleNamespace(get_transform=lambda *a:(calls.append(a) or ([1.,2.,3.,0.,0.,0.,1.],stamp)))
        camera.publisher = types.SimpleNamespace(register_sensor=lambda raw:specs.update({json.loads(raw)['id']:json.loads(raw)}),
            publish_sensor=lambda identity,t,data,raw:events.update({identity:(t,json.loads(data) if data else None,raw)}))
        with tempfile.TemporaryDirectory() as root:
            camera.root = Path(root)
            with patch.object(sensors,'camera_preview',return_value=(b'preview',640,496,dict(calibration,width=640,height=496))):
                camera.camera(name,None,None)
        return calls,specs,events

    def test_every_camera_shares_one_thin_pose_with_its_preview(self):
        for name in sensors.CAMERAS:
            with self.subTest(camera=name):
                calls,specs,events = self.capture(name)
                source = 'camera.'+name
                self.assertEqual(calls,[('base_link',name+'_optical',123,0)])
                self.assertEqual(set(events),{source,source+'-preview',source+'.pose'})
                t,pose,raw = events[source+'.pose']
                self.assertEqual(t,events[source][0]);self.assertEqual(raw,b'')
                self.assertEqual(set(pose),{'contract','tracking'})
                self.assertEqual(pose['contract']['from_frame']['frame_id'],name+'_optical')
                self.assertEqual(pose['contract']['to_frame']['frame_id'],'robot.base')
                self.assertEqual(specs[source+'.pose']['payload']['datatype'],'auki.odometry-pose')
                for product in [source,source+'-preview']:
                    self.assertEqual(specs[product]['metadata']['pose_product_id'],source+'.pose-history')
                    self.assertIn('calibration',specs[product]['metadata'])
                self.assertNotIn('calibration',specs[source+'.pose']['metadata'])

    def test_missing_capture_transform_keeps_plain_video(self):
        _,specs,events = self.capture(stamp=122)
        self.assertEqual(set(events),{'camera.head_left','camera.head_left-preview'})
        self.assertNotIn('camera.head_left.pose',specs)


class PoseTransport(unittest.TestCase):
    def test_capture_mailbox_retains_pose_history_but_coalesces_images(self):
        spec = importlib.util.spec_from_file_location('capture', Path(__file__).resolve().parents[1] / 'scripts/capture-runner.py')
        capture = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(capture)
        publisher = capture.Publisher()
        publisher.register_sensor(json.dumps({'id':'sensor-pose','payload':{'datatype':'auki.odometry-pose'}}))
        for t in range(3000):
            publisher.publish_sensor('sensor-pose',t,'{}',b'')
            publisher.publish_sensor('image',t,'',b'jpeg')
        with patch.object(capture,'emit') as emit:
            publisher.drain()
        events = [c.args[0] for c in emit.call_args_list]
        self.assertEqual(len(events),2050)
        self.assertEqual(events[1]['timestamp_ns'],952)
        self.assertEqual(events[-2]['timestamp_ns'],2999)
        self.assertEqual(events[-1]['sensor_id'],'image')

if __name__=='__main__': unittest.main()
