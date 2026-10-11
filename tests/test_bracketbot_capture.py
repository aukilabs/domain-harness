import sys
import unittest
import importlib.util
import io
import os
import threading
import time
import tempfile
from types import SimpleNamespace
from unittest.mock import patch
from pathlib import Path
import numpy as np
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from bracketbot_capture import (FramePairs, NewSessionRequired, PoseGate, calibration,
                               fresh_jpeg, optical_points, rotation_quaternion, sampled_points,
                               wrist_camera_spec, right_camera_transform, RIGHT_CAMERA_FRAME,
                               head_camera_specs, CAMERA_FRAME, CaptureMailbox)
from bracketbot_capture import corrected_portal_poses, POSE_HISTORY_DTYPE
from bracketbot_capture import trajectory_correction


def record(t, **kwargs):
    return dict(timestamp=np.datetime64(t, 'ns'), **kwargs)


def valid_pose(t, pgo=1):
    return record(t, pos=[1., 2., 0.], quat=[0., 0., 0., 1.], pgo_count=pgo)


def health(t, **changes):
    values = dict(vo_lost=False, stalled=False, degraded=False, localized=True)
    values.update(changes)
    return record(t, **values)


class BracketbotCaptureTests(unittest.TestCase):
    def test_capture_reader_cleanup_cannot_swallow_transport_failure(self):
        spec = importlib.util.spec_from_file_location('capture_bracketbot',
            Path(__file__).resolve().parents[1] / 'scripts/capture-bracketbot.py')
        capture = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(capture)
        closed = []
        class Reader:
            def __init__(self, name, **_): self.name = name
            def __enter__(self): return self
            def __exit__(self, *_): closed.append(self.name); return True
            def ready(self):
                t = time.time_ns()
                if self.name == 'slam.health': self.data = health(t)
                elif self.name == 'slam.pose': self.data = valid_pose(t)
                else: return False
                return True
        class SensorPath:
            def __truediv__(self, _): return self
            def stat(self): return SimpleNamespace(st_ino=1)
            def open(self, *_): return io.BytesIO(bytes(12))
        path = SensorPath()
        def fail(_): raise RuntimeError('transport closed')
        with patch.object(capture, 'Path', lambda *_: path), self.assertRaisesRegex(RuntimeError, 'transport closed'):
            capture.acquire(Reader, CaptureMailbox(), fail, threading.Event(), os.getppid())
        self.assertEqual(len(closed),6)

    def test_slow_image_consumer_does_not_coalesce_dense_pose_captures(self):
        box = CaptureMailbox(); gate = PoseGate()
        for i in range(100):
            t = 1_000_000_000 + i*33_000_000
            admitted = gate.accept(valid_pose(t), health(t), t)
            box.update_pose(gate, admitted, health(t))
            box.image('camera.head', t, b'jpeg')
            box.points(t, [[0,0,1]], [[255,0,0]])
        state = box.snapshot()
        self.assertEqual(state['poses'],100)
        self.assertEqual(state['max_pose_gap_ms'],33)
        self.assertLessEqual(len(box.pairs.frames),48)
        self.assertEqual(len(state['previews']),1)
        self.assertEqual(box.take_scan(0,t)[0],t)

    def test_pending_image_cannot_cross_correction_or_tracking_loss(self):
        box = CaptureMailbox(); gate = PoseGate(); t = 1_000_000_000
        box.update_pose(gate,gate.accept(valid_pose(t),health(t),t),health(t))
        box.image('camera.head',t,b'old'); box.points(t,[],[])
        self.assertTrue(box.scan_is_current(t,t))
        box.update_pose(gate,gate.accept(valid_pose(t+100,2),health(t+100),t+100),health(t+100))
        self.assertFalse(box.scan_is_current(t,t+100))
        self.assertIsNone(box.take_scan(0,t+100))
        box.image('camera.head',t,b'late old'); box.points(t,[],[])
        self.assertIsNone(box.take_scan(0,t+100))
        box.image('camera.head',t+100,b'new'); box.points(t+100,[],[])
        self.assertEqual(box.take_scan(0,t+100)[0],t+100)
        self.assertFalse(box.scan_is_current(t+100,t+900_000_000))
        lost=health(t+200,vo_lost=True)
        box.update_pose(gate,gate.accept(valid_pose(t+200,2),lost,t+200),lost)
        self.assertFalse(box.scan_is_current(t+100,t+200))

    def test_unmatched_and_stale_frames_are_not_emitted_as_scans(self):
        box = CaptureMailbox(); gate=PoseGate(); t=1_000_000_000
        box.update_pose(gate,gate.accept(valid_pose(t),health(t),t),health(t))
        box.image('camera.head',t,b'jpeg'); box.points(t+1,[],[])
        self.assertIsNone(box.take_scan(0,t+1))
        box.points(t,[],[])
        self.assertIsNone(box.take_scan(0,t+900_000_000))
        self.assertEqual(box.snapshot()['stale_scans'],1)

    def test_head_eyes_have_separate_geometry_without_slam_or_depth(self):
        context = dict(clock={'peer_id':'robot'}, session_id='test',
                       camera_frame_definition={'frame_id':CAMERA_FRAME},
                       base_frame_definition={'frame_id':'robot.base'})
        eyes = [(name, np.eye(3), dict(sensor_frame=frame, calibration_source='test'))
                for name, frame in [('camera.head',CAMERA_FRAME),('camera.head.right',RIGHT_CAMERA_FRAME)]]
        specs = head_camera_specs(context,1280,960,eyes)
        self.assertEqual(len(specs),4)
        for i, (name, _, calibration) in enumerate(eyes):
            pose, camera = specs[2*i:2*i+2]
            self.assertEqual(camera['id'],name)
            self.assertEqual(camera['frame_definition']['frame_id'],calibration['sensor_frame'])
            self.assertEqual(pose['metadata']['pose_contract']['from_frame'],camera['frame_definition'])
            self.assertEqual(camera['metadata']['pose_product_id'],name+'.pose-history')
        self.assertEqual(context['camera_frame_definition']['frame_id'],CAMERA_FRAME)

    def test_right_eye_baseline_direction_units_and_base_rotation(self):
        left = np.array([[0,0,1,.1],[1,0,0,.2],[0,1,0,1.55]],float)
        right = right_camera_transform(left, np.eye(3), np.eye(3), np.eye(3), [-.0645,0,0])
        np.testing.assert_allclose(right[:,3], [.1,.2645,1.55])
        point_right = np.array([[.2,.1,2.]])
        base = point_right @ right[:,:3].T + right[:,3]
        point_left = optical_points(base,left)
        np.testing.assert_allclose(point_left, point_right + [.0645,0,0])
        k,spec = calibration(np.eye(3),right,640,480,640,480,b'cal',RIGHT_CAMERA_FRAME)
        self.assertEqual(spec['sensor_frame'],RIGHT_CAMERA_FRAME)
        with self.assertRaises(ValueError):
            right_camera_transform(left,np.eye(3),np.eye(3),np.eye(3),[-64.5,0,0])

    def test_wrist_feed_does_not_claim_head_calibration_or_pose(self):
        context = dict(clock={'peer_id': 'robot'}, camera_frame_definition={
            'frame_id': 'head', 'convention': 'ros_optical'})
        left, right = [wrist_camera_spec(context, side) for side in ('left', 'right')]
        self.assertNotEqual(left['frame_definition'], right['frame_definition'])
        self.assertEqual(context['camera_frame_definition']['frame_id'], 'head')
        self.assertNotIn('calibration', left['metadata'])
        self.assertNotIn('pose_product_id', left['metadata'])
        t = 1_000_000_000
        jpeg = record(t, jpeg_len=3, jpeg=np.array([1, 2, 3, 4], np.uint8))
        self.assertEqual(fresh_jpeg(jpeg, t), b'\x01\x02\x03')
        self.assertIsNone(fresh_jpeg(jpeg, t+900_000_000))
        with self.assertRaises(ValueError):
            fresh_jpeg(record(t, jpeg_len=5, jpeg=jpeg['jpeg']), t)

    def test_native_base_cloud_roundtrips_to_calibrated_pixels(self):
        # Optical +Z looks forward (+Y) in the BBOS base; +Y points down.
        transform = np.array([[1, 0, 0, 0], [0, 0, 1, .2], [0, -1, 0, 1.55]], float)
        k = np.array([[500, 0, 320], [0, 500, 240], [0, 0, 1]], float)
        camera = np.array([[.4, -.2, 2.], [-.3, .5, 3.]])
        base = camera @ transform[:, :3].T + transform[:, 3]
        restored = optical_points(base.astype(np.float16), transform)
        pixels = restored @ k.T
        pixels = pixels[:, :2] / pixels[:, 2:]
        np.testing.assert_allclose(pixels, [[420, 190], [270, 323.3333]], atol=.25)
        scaled, spec = calibration(k, transform, 320, 240, 640, 480, b'calibration')
        np.testing.assert_allclose(scaled, [[250, 0, 160], [0, 250, 120], [0, 0, 1]])
        np.testing.assert_allclose(spec['sensor_to_base']['rotation_xyzw'], [-2**-.5, 0, 0, 2**-.5])

    def test_rotation_validation_rejects_nonrigid_mount(self):
        with self.assertRaises(ValueError): rotation_quaternion(np.diag([1., 1., 2.]))
        np.testing.assert_allclose(rotation_quaternion([[0,-1,0],[1,0,0],[0,0,1]]),
                                   [0,0,2**-.5,2**-.5], atol=1e-10)

    def test_pairs_require_exact_timestamps_and_are_bounded(self):
        pairs = FramePairs(3)
        pairs.add('rgb', 100, 'image')
        pairs.add('points', 101, 'wrong')
        self.assertIsNone(pairs.take(0))
        pairs.add('points', 100, 'correct')
        self.assertEqual(pairs.take(0), (100, 'image', 'correct'))
        for t in range(200, 220): pairs.add('rgb', t, t)
        self.assertEqual(len(pairs.frames), 3)
        self.assertIsNone(pairs.take(220))

    def test_stale_or_lost_pose_never_becomes_fresh_by_repolling(self):
        gate = PoseGate()
        t = 1_000_000_000
        self.assertIsNotNone(gate.accept(valid_pose(t), health(t), t))
        self.assertIsNone(gate.accept(valid_pose(t), health(t+900_000_000), t+900_000_000))
        self.assertFalse(gate.tracking)
        for flag in ['vo_lost', 'stalled']:
            self.assertIsNone(PoseGate().accept(valid_pose(t), health(t, **{flag:True}), t))
        self.assertIsNone(PoseGate().accept(valid_pose(t), health(t, localized=False), t))
        self.assertIsNone(PoseGate().accept(valid_pose(t), health(1), t))

    def test_degraded_quality_preserves_bbos_valid_pose_contract(self):
        gate = PoseGate(); t = 1_000_000_000
        self.assertIsNotNone(gate.accept(valid_pose(t), health(t, degraded=True), t))
        self.assertTrue(gate.tracking)
        self.assertIsNone(gate.accept(valid_pose(t), health(t, degraded=True, vo_lost=True), t))
        self.assertFalse(gate.tracking)

    def test_clock_regression_still_requires_new_session(self):
        gate = PoseGate(); t = 1_000_000_000
        gate.accept(valid_pose(t), health(t), t)
        with self.assertRaises(NewSessionRequired):
            gate.accept(valid_pose(t-10), health(t), t)

    def test_pose_graph_change_emits_one_barrier_without_losing_tracking(self):
        gate = PoseGate(); t = 1_000_000_000
        gate.accept(valid_pose(t), health(t), t)
        self.assertIsNone(gate.correction)
        for n in [2, 3]:
            t += 100_000_000
            result = gate.accept(valid_pose(t, n), health(t), t)
            self.assertEqual(gate.correction, dict(kind='map_correction', timestamp_ns=t,
                             pgo_count=n, pose=result[1]))
            self.assertTrue(gate.tracking)
            self.assertIsNone(gate.accept(valid_pose(t, n), health(t), t))
            self.assertIsNone(gate.correction)

    def test_pose_graph_reset_requires_a_new_capture_session(self):
        gate=PoseGate(); t=1_000_000_000
        gate.accept(valid_pose(t,3),health(t),t)
        with self.assertRaises(NewSessionRequired):
            gate.accept(valid_pose(t+10,0),health(t+10),t+10)

    def test_corrected_history_interpolates_detector_capture_not_current_pose(self):
        rows=np.zeros(2,dtype=POSE_HISTORY_DTYPE)
        rows['ts_ns']=[1_000_000_000,1_200_000_000]
        rows['pos']=[[2,0,0],[4,0,0]]; rows['quat']=[[0,0,0,1],[0,0,1,0]]
        raw=b'BBPOSE1\0'+np.array([40,2,5,3],dtype='<i8').tobytes()+rows.tobytes()
        evidence={'portal':dict(capture_timestamp_ns=1_100_000_000,base_odometry_pose={'translation':[0,0,0]})}
        result=corrected_portal_poses(raw,3,evidence)['portal']
        self.assertEqual(result['timestamp_ns'],1_100_000_000)
        self.assertEqual(result['pose']['translation'],[3,0,0])
        np.testing.assert_allclose(result['pose']['rotation_xyzw'],[0,0,2**-.5,2**-.5])
        self.assertEqual(corrected_portal_poses(raw,4,evidence),{})
        evidence['portal']['capture_timestamp_ns']=1_300_000_000
        self.assertEqual(corrected_portal_poses(raw,3,evidence),{})
        self.assertEqual(corrected_portal_poses(raw[:-1],3,evidence),{})

    def test_history_must_match_pgo_and_cover_retained_depth_before_replay(self):
        import json
        rows=np.zeros(2,dtype=POSE_HISTORY_DTYPE)
        rows['ts_ns']=[1_000_000_000,1_200_000_000];rows['quat'][:,3]=1
        with tempfile.TemporaryDirectory() as directory:
            control=Path(directory)/'control.json';history=Path(directory)/'history'
            control.write_text(json.dumps(dict(registries=dict(local_mapping=dict(observed_portals={},retained_depth_timestamps=[1_100_000_000])))))
            history.write_bytes(b'BBPOSE1\0'+np.array([40,2,5,2],dtype='<i8').tobytes()+rows.tobytes())
            self.assertFalse(trajectory_correction(dict(pgo_count=3),control,history)['history_ready'])
            history.write_bytes(b'BBPOSE1\0'+np.array([40,2,5,3],dtype='<i8').tobytes()+rows.tobytes())
            result=trajectory_correction(dict(pgo_count=3),control,history)
            self.assertTrue(result['history_ready']);self.assertIn('1100000000',result['corrected_depth'])
            control.write_text(json.dumps(dict(registries=dict(local_mapping=dict(observed_portals={},retained_depth_timestamps=[1_300_000_000])))))
            self.assertFalse(trajectory_correction(dict(pgo_count=3),control,history)['history_ready'])

    def test_correction_during_loss_waits_for_a_fresh_valid_pose(self):
        gate = PoseGate(); t = 1_000_000_000
        gate.accept(valid_pose(t), health(t), t)
        self.assertIsNone(gate.accept(valid_pose(t+10, 2), health(t+10, vo_lost=True), t+10))
        self.assertIsNone(gate.correction)
        self.assertIsNotNone(gate.accept(valid_pose(t+20, 2), health(t+20), t+20))
        self.assertEqual(gate.correction['pgo_count'], 2)

    def test_recovery_and_duplicate_capture_do_not_republish_old_pose(self):
        gate = PoseGate(); t = 1_000_000_000
        gate.accept(valid_pose(t), health(t), t)
        self.assertIsNone(gate.accept(valid_pose(t), health(t), t+1))
        gate.accept(valid_pose(t), health(t+1, vo_lost=True), t+1)
        self.assertFalse(gate.tracking)
        result = gate.accept(valid_pose(t+20), health(t+20), t+20)
        self.assertEqual(result[0], t+20)
        self.assertTrue(gate.tracking)

    def test_depth_filter_bounds_bandwidth_and_preserves_rgb_pairs(self):
        points = np.tile([.1, .2, 2.], (1500, 1))
        colors = np.arange(4500, dtype=np.int64).astype(np.uint8).reshape(-1, 3)
        points[:4] = [[np.nan,0,1], [0,0,0], [0,0,6], [4,0,4]]
        hits, rgb = sampled_points(points, colors, 10_000_000)
        self.assertLessEqual(len(hits), 384)
        self.assertEqual(len(hits), len(rgb))
        self.assertTrue(all(p == [.1,.2,2.] for p in hits))
        expected = colors[4:][2::4][:384].tolist()
        self.assertEqual(rgb, expected)


if __name__ == '__main__': unittest.main()
