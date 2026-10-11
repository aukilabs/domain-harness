#!/usr/bin/env python3
"""Supervised search/reach policy; each motion mode requires its execution flag.

Search is wrist-only. Calibrated reaching uses authenticated phone positions and
a bounded full-arm path. Faults latch until an explicit service restart.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from bracketbot_behavior import SearchLatch, decide


def reach_exit_ok(report,returncode,cancelled):
    # Localization can disappear during Python/numpy startup, before the helper
    # installs its handler or emits a report. Our SIGTERM then exits directly;
    # no motor writer could have been acquired before the initial report.
    if cancelled and returncode==-signal.SIGTERM and not report:return True
    return (returncode==0 and report.get('phase')=='restored'
            and report.get('reason') in ('cancelled','target_changed_or_stale','gesture_complete'))


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--state', type=Path, required=True)
    ap.add_argument('--supervisor-pid', type=int, required=True)
    ap.add_argument('--execute-search', action='store_true')
    ap.add_argument('--execute-phone-reach', action='store_true')
    args = ap.parse_args()
    state = args.state
    stopped = False
    child = None
    child_kind = None
    cancelled = False
    search_since = None
    latch = SearchLatch()
    def stop(*_):
        nonlocal stopped
        stopped = True
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    def write_status(report):
        report.update(pid=os.getpid(), updated_ns=time.time_ns(),
                      search_execution_enabled=args.execute_search,
                      pointing_execution_enabled=args.execute_phone_reach)
        tmp = state/'arm-behavior.status.tmp'
        tmp.write_text(json.dumps(report, allow_nan=False)+'\n')
        tmp.replace(state/'arm-behavior.status.json')
    def cancel():
        nonlocal cancelled
        if child and child.poll() is None and not cancelled:
            child.terminate(); cancelled = True
    with (state/'arm-behavior.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            while not stopped and os.getppid() == args.supervisor_pid:
                if child and child.poll() is not None:
                    try:
                        report = json.loads((state/({'phone':'phone-reach.json','note':'note-reach.json'}.get(child_kind,'portal-stance.json'))).read_text())
                        if report.get('pid') != child.pid: report = {}
                    except (OSError, ValueError): report = {}
                    if child_kind in ('phone','note'):
                        if not reach_exit_ok(report,child.returncode,cancelled):
                            latch.fault=f'Reach exited {child.returncode}: {report.get("reason", "no helper report")}; inspect and explicitly rearm'
                    print(json.dumps(dict(event='motion_exit',pid=child.pid,kind=child_kind,
                                          returncode=child.returncode,cancelled=cancelled,report=report)),flush=True)
                    if child_kind not in ('phone','note'):
                        latch.completed(report, cancelled=cancelled)
                    child = None; child_kind = None; cancelled = False
                    search_since = None
                try:
                    path = state/'robot-control.json'
                    stamp = path.stat().st_mtime_ns
                    scene = json.loads(path.read_text())
                    home = json.loads((state/'home-connection.json').read_text())
                    runner = json.loads((state/'supervisor-live.json').read_text())
                    now = time.time_ns()
                    pose = scene['registries']['local_mapping'].get('robot_pose')
                    decision = decide(scene, home['frame'], scene_age_ms=(now-stamp)/1e6,
                        robot_pose_age_ms=(now-pose['capture_timestamp_ns'])/1e6 if pose else None,
                        runner_alive=(runner['running'] and runner['pid'] == args.supervisor_pid
                                      and 0 <= now-runner['updated_ns'] < 1_000_000_000),
                        fault=latch.fault)
                except (OSError, ValueError, KeyError, TypeError):
                    decision = decide(None, '', scene_age_ms=None, robot_pose_age_ms=None,
                                      runner_alive=False, fault=latch.fault)
                desired_kind = ('search' if decision['search_requested'] and args.execute_search else
                                'phone' if decision.get('phone_requested') and args.execute_phone_reach else None)
                note_path=state/'reach-target.json'
                if note_path.exists():
                    try:
                        request=json.loads(note_path.read_text())
                        if not request['request_id']:raise ValueError('missing request identity')
                        if desired_kind=='phone':desired_kind='note'
                    except (OSError,ValueError,KeyError):
                        desired_kind=None
                        decision.update(phase='note_target_invalid',reason='Invalid note request; phone fallback disabled')
                if desired_kind in ('phone','note'):
                    decision.update(phase=desired_kind+'_reach_ready',reason='Reaching toward selected note' if desired_kind=='note' else 'Selecting nearest live localized phone')
                if desired_kind is None or (child and child_kind!=desired_kind):
                    search_since = None
                    cancel()
                elif child is None:
                    # Require a stable runner heartbeat before acquiring arms.
                    if search_since is None: search_since = time.monotonic()
                    if time.monotonic()-search_since < 2:
                        write_status(dict(decision, phase='waiting_for_stable_runner'))
                        time.sleep(.1)
                        continue
                    write_status(dict(decision, phase='starting_'+desired_kind))
                    script = Path(__file__).with_name('seek-bracketbot-portal.py' if desired_kind=='search' else 'reach-bracketbot-phone.py')
                    wrapper = Path(__file__).with_name('bracketbot-python.py')
                    child = subprocess.Popen([sys.executable, str(wrapper), '--daemon', 'arm_left',
                        str(script), '--state', str(state), '--execute',
                        '--controller-pid', str(os.getpid())] +
                        (['--hold-until-calibrated'] if desired_kind=='search' else []) +
                        (['--note-target',str(note_path)] if desired_kind=='note' else []), stdin=subprocess.DEVNULL)
                    child_kind=desired_kind
                    cancelled = False
                decision['search_process_pid'] = child.pid if child else None
                decision['motion_process_pid'] = child.pid if child else None
                decision['child_kind'] = child_kind
                decision['actuation_enabled'] = bool(child and not cancelled)
                if cancelled: decision['phase'] = 'stopping_'+str(child_kind)
                write_status(decision)
                time.sleep(.1)
        finally:
            cancel()
            if child:
                deadline = time.monotonic()+65
                while child.poll() is None and time.monotonic() < deadline:
                    write_status(dict(phase='stopping_search', actuation_enabled=False))
                    time.sleep(.1)
                if child.poll() is None:
                    child.kill(); child.wait()
                    latch.fault = 'Search did not relinquish control within shutdown deadline'
            write_status(dict(phase='stopped', actuation_enabled=False, reason=latch.fault))


if __name__ == '__main__':
    main()
