import copy
from pathlib import Path
import sys
import unittest
import json
import os
import subprocess
import tempfile
import time
import signal
import importlib.util
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_behavior import SearchLatch, alignment_valid, decide, supervisor_fresh
from test_bracketbot_pointing import fixtures
spec=importlib.util.spec_from_file_location('controller',Path(__file__).resolve().parents[1]/'scripts/control-bracketbot-behavior.py')
controller=importlib.util.module_from_spec(spec);spec.loader.exec_module(controller)


class BehaviorTests(unittest.TestCase):
    def test_cancel_during_startup_is_not_a_hardware_fault(self):
        self.assertTrue(controller.reach_exit_ok({},-signal.SIGTERM,True))
        self.assertFalse(controller.reach_exit_ok({},-signal.SIGTERM,False))
        self.assertFalse(controller.reach_exit_ok({},1,True))
        self.assertFalse(controller.reach_exit_ok(dict(phase='reaching'),-signal.SIGTERM,True))
        self.assertFalse(controller.reach_exit_ok(dict(phase='fault',reason='joint bounds'),1,True))
        self.assertTrue(controller.reach_exit_ok(dict(phase='restored',reason='cancelled'),0,True))

    def test_motor_watchdog_rejects_dead_replaced_frozen_and_future_controller(self):
        now=5_000_000_000
        self.assertTrue(supervisor_fresh(dict(pid=10,updated_ns=now),10,10,now))
        for heartbeat,parent in [({},10),(dict(pid=11,updated_ns=now),10),
                                 (dict(pid=10,updated_ns=now),1),
                                 (dict(pid=10,updated_ns=now-1_000_000_000),10),
                                 (dict(pid=10,updated_ns=now+1),10)]:
            self.assertFalse(supervisor_fresh(heartbeat,10,parent,now))

    def setUp(self):
        self.scene, _, _ = fixtures()
        self.scene['registries']['local_mapping']['status'] = 'mapping'

    def decide(self, scene=None, **kwargs):
        args = dict(scene_age_ms=0, robot_pose_age_ms=0, runner_alive=True)
        args.update(kwargs)
        return decide(scene or self.scene, 'home', **args)

    def test_search_alignment_loss_and_recovery(self):
        a = self.scene['registries']['frame_transform']
        a['validity'] = 'unavailable'
        self.assertTrue(self.decide()['search_requested'])
        a['validity'] = 'valid'
        self.assertEqual(self.decide()['phase'], 'pointing_blocked')
        self.assertFalse(self.decide()['actuation_enabled'])
        a['validity'] = 'stale'
        self.assertTrue(self.decide()['search_requested'])

    def test_alignment_does_not_expire_when_qr_leaves_view(self):
        # Evidence is intentionally old; fresh robot tracking keeps mapper's
        # published alignment valid, so we must not flap back to search.
        self.assertTrue(alignment_valid(self.scene, 'home'))
        self.assertFalse(self.decide()['search_requested'])

    def test_stale_aligned_pose_does_not_start_search_or_pointing(self):
        for kwargs in [dict(scene_age_ms=501), dict(robot_pose_age_ms=801),
                       dict(runner_alive=False), dict(scene_age_ms=-1),
                       dict(robot_pose_age_ms=float('nan'))]:
            self.assertEqual(self.decide(**kwargs)['phase'], 'waiting_for_tracking')
        self.scene['registries']['local_mapping']['status'] = 'tracking_lost'
        self.assertFalse(self.decide()['search_requested'])

    def test_unlocalized_wrist_search_does_not_need_slam_pose(self):
        m=self.scene['registries']['local_mapping']
        m['status']='tracking_lost'; m['robot_pose']=None
        self.scene['registries']['frame_transform']['validity']='unavailable'
        result=self.decide(scene_age_ms=0,robot_pose_age_ms=None)
        self.assertTrue(result['search_requested'])
        self.assertFalse(result['actuation_enabled'])  # actuator still performs its own preflight
        self.assertFalse(self.decide(runner_alive=False)['search_requested'])
        self.assertFalse(self.decide(scene_age_ms=3000,robot_pose_age_ms=None)['search_requested'])

    def test_wrong_frame_owner_and_invalid_transform_are_not_calibration(self):
        for key, value in [('authority_peer_id','other'), ('target_frame',{'id':'other'}),
                           ('source_frame',{'id':'old-session'})]:
            s=copy.deepcopy(self.scene); s['registries']['frame_transform'][key]=value
            self.assertFalse(alignment_valid(s, 'home'))
        s=copy.deepcopy(self.scene)
        s['registries']['frame_transform']['transform']['translation']=[float('nan'),0,0]
        self.assertFalse(alignment_valid(s, 'home'))

    def test_faults_latch_and_never_retry_motion(self):
        for report in [{}, dict(phase='fault'), dict(phase='restored', reason='temperature_margin'),
                       dict(phase='restored', reason='timeout'),
                       dict(phase='restored', reason='cancelled')]:
            latch=SearchLatch(); latch.completed(report, cancelled=False)
            self.assertIsNotNone(latch.fault)
            self.assertEqual(self.decide(fault=latch.fault)['phase'], 'fault')
        for reason in ('calibrated','cancelled'):
            latch=SearchLatch(); latch.completed(dict(phase='restored',reason=reason), cancelled=True)
            self.assertIsNone(latch.fault)
        latch=SearchLatch(); latch.completed(dict(phase='already_calibrated'),cancelled=False)
        self.assertIsNone(latch.fault)

    def test_runner_supervisor_owns_and_stops_read_only_behavior(self):
        script=Path(__file__).resolve().parents[1]/'scripts/supervise-bracketbot.py'
        with tempfile.TemporaryDirectory() as directory:
            state=Path(directory)
            runner=state/'fake-runner'
            runner.write_text('#!'+sys.executable+'\nimport time\ntime.sleep(30)\n')
            runner.chmod(0o700)
            child=subprocess.Popen([sys.executable,str(script),'--state',str(state),'--binary',str(runner)],
                                   stdout=subprocess.DEVNULL)
            try:
                until=time.monotonic()+4
                report=None
                while time.monotonic()<until:
                    try:
                        report=json.loads((state/'arm-behavior.status.json').read_text())
                        if report['phase']=='waiting_for_tracking': break
                    except (OSError,ValueError): pass
                    time.sleep(.03)
                self.assertIsNotNone(report)
                self.assertFalse(report['search_execution_enabled'])
                child.terminate(); child.wait(timeout=5)
                report=json.loads((state/'arm-behavior.status.json').read_text())
                self.assertEqual(report['phase'],'stopped')
                self.assertFalse((state/'runner.pid').exists())
            finally:
                if child.poll() is None: child.terminate(); child.wait(timeout=5)

    def test_controller_read_only_lifecycle_and_stale_supervisor(self):
        script=Path(__file__).resolve().parents[1]/'scripts/control-bracketbot-behavior.py'
        with tempfile.TemporaryDirectory() as directory:
            state=Path(directory)
            def refresh():
                now=time.time_ns()
                self.scene['registries']['local_mapping']['robot_pose']['capture_timestamp_ns']=now
                for name,value in [('robot-control.json',self.scene),
                                   ('home-connection.json',{'frame':'home'}),
                                   ('supervisor-live.json',dict(pid=os.getpid(),running=True,updated_ns=now))]:
                    tmp=state/(name+'.tmp'); tmp.write_text(json.dumps(value)); tmp.replace(state/name)
            def wait_for(phase, refresh_inputs=True):
                until=time.monotonic()+4
                while time.monotonic()<until:
                    if refresh_inputs: refresh()
                    try:
                        report=json.loads((state/'arm-behavior.status.json').read_text())
                        if report['phase']==phase: return report
                    except (OSError,ValueError): pass
                    time.sleep(.03)
                self.fail('Controller did not reach '+phase)
            refresh()
            child=subprocess.Popen([sys.executable,str(script),'--state',str(state),
                                    '--supervisor-pid',str(os.getpid())],stdout=subprocess.DEVNULL)
            try:
                report=wait_for('pointing_blocked')
                self.assertFalse(report['actuation_enabled'])
                (state/'reach-target.json').write_text(json.dumps(dict(request_id='completed-note')))
                (state/'note-reach.json').write_text(json.dumps(dict(request_id='completed-note',phase='restored',reason='gesture_complete')))
                report=wait_for('pointing_blocked')  # old completion never consumes a persistent target
                self.assertIsNone(report['motion_process_pid'])
                (state/'reach-target.json').write_text(json.dumps(dict(request_id='another-note')))
                wait_for('pointing_blocked')  # new runtime request, same controller PID
                (state/'reach-target.json').unlink()
                self.scene['registries']['frame_transform']['validity']='unavailable'
                report=wait_for('searching')
                self.assertIsNone(report['search_process_pid'])
                self.assertFalse(report['search_execution_enabled'])
                self.assertFalse((state/'portal-stance.json').exists())
                wait_for('waiting_for_tracking',refresh_inputs=False)
                child.terminate(); child.wait(timeout=4)
                self.assertEqual(json.loads((state/'arm-behavior.status.json').read_text())['phase'],'stopped')
            finally:
                if child.poll() is None: child.kill(); child.wait()


if __name__ == '__main__': unittest.main()
