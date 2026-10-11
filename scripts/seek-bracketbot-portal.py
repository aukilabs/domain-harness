#!/usr/bin/env python3
"""One bounded arm search/restore cycle; dry-run unless --execute is supplied.

Run via bracketbot-python.py on BBOS. This is deliberately separate from the
read-only capture worker and its automatic restart loop. No base commands.
"""
import argparse
import contextlib
import fcntl
import json
import os
from pathlib import Path
import signal
import time

import numpy as np
from bracketbot_capture import timestamp
from bracketbot_stance import calibrated, ease, hand_axis, plan_wrist
from bracketbot_behavior import supervisor_fresh


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--state', type=Path, required=True)
    ap.add_argument('--portal', default='HNY3JGNLRAE')
    ap.add_argument('--execute', action='store_true')
    ap.add_argument('--max-seconds', type=float, default=300.)
    ap.add_argument('--hold-until-calibrated', action='store_true',
                    help='Supervised search: hold until alignment, cancellation or safety stop')
    ap.add_argument('--controller-pid', type=int)
    args = ap.parse_args()
    if not 1 <= args.max_seconds <= 300: ap.error('--max-seconds must be 1..300')
    if args.hold_until_calibrated and not args.controller_pid:
        ap.error('--hold-until-calibrated requires a supervising controller')
    from bbos import Config, Reader, Type, Writer
    configs = {side: Config('arm_'+side) for side in ('left', 'right')}
    args.state.mkdir(parents=True, exist_ok=True)
    status_path = args.state/'portal-stance.json'
    scene_path = args.state/'robot-control.json'
    capture_status_path = args.state/'local-scene.status.json'
    def capture_status():
        if time.time()-capture_status_path.stat().st_mtime > 5:
            raise RuntimeError('runner status is stale')
        return json.loads(capture_status_path.read_text())
    def scene():
        if not scene_path.exists(): return None
        return json.loads(scene_path.read_text())
    capture = capture_status()
    home_frame = json.loads((args.state/'home-connection.json').read_text())['frame']
    initial_scene = scene()
    frame = initial_scene['frame'] if initial_scene else capture['frame']
    # Mapping can establish its origin before the independent camera catalog
    # worker publishes its first source. The origin owns the capture clock.
    clock = ((initial_scene or {}).get('registries',{}).get('local_mapping',{}).get('origin') or {}).get('capture_clock')
    if clock is None:
        sources=capture.get('sensors',{}).get('sources',[])
        if not sources:raise RuntimeError('capture clock not ready')
        clock=sources[0]['clock']
    baseline = (initial_scene['registries']['local_mapping']['observed_portals'].get(args.portal, {}).get('observations', 0)
                if initial_scene and initial_scene['frame'] == frame else 0)
    report = dict(pid=os.getpid(), portal=args.portal, frame=frame, execute=args.execute,
                  phase='preflight', started_ns=time.time_ns())
    def status(phase, **details):
        report.update(phase=phase, updated_ns=time.time_ns(), **details)
        if args.execute:
            tmp = status_path.with_suffix('.tmp')
            tmp.write_text(json.dumps(report, indent=2)+'\n'); tmp.replace(status_path)
        print(json.dumps(report), flush=True)
    with (args.state/'portal-stance.lock').open('a') as lock, contextlib.ExitStack() as readers_stack:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if calibrated(initial_scene, args.portal, time.time_ns()-2_000_000_000,
                      time.time_ns(), frame, clock, home_frame):
            status('already_calibrated')
            return
        readers = {side: readers_stack.enter_context(Reader('arm_'+side+'.state', keeptime=False)) for side in configs}
        imu = readers_stack.enter_context(Reader('imu.orientation', keeptime=False))
        battery = readers_stack.enter_context(Reader('drive.status', keeptime=False))
        low_voltage = Config('base').low_battery_v
        def fresh(reader, max_age=.25):
            reader.ready()
            d = reader.data.copy()
            if not -.1 <= (time.time_ns()-timestamp(d))/1e9 <= max_age:
                raise RuntimeError('stale BBOS feedback; relinquishing to factory controller')
            return d
        def healthy(arm_max_age=.25):
            if np.max(np.abs(fresh(imu)['rpy'][:2])) > 10:
                raise RuntimeError('robot tilted; yielding to factory safety controller')
            # drive.status is the factory's slow (~10 s) battery telemetry.
            if float(fresh(battery, 15)['voltage']) < low_voltage:
                raise RuntimeError('battery below factory low-voltage threshold')
            states = {side: fresh(reader, arm_max_age) for side, reader in readers.items()}
            for side, d in states.items():
                for field in ('pos', 'vel', 'temp', 'current'):
                    if not np.isfinite(d[field]).all(): raise RuntimeError('invalid arm feedback')
                if np.max(d['temp']) >= 50 or np.any(np.abs(d['current']) >= np.asarray(configs[side].hard_current_limit)*.8):
                    raise RuntimeError('arm protection margin exceeded; yielding to factory controller')
            return states
        states = healthy()
        start, target, fk = {}, {}, {}
        for side, cfg in configs.items():
            if np.max(np.abs(states[side]['vel'])) > .02:
                raise RuntimeError('arm is moving; refusing to take control')
            start[side] = states[side]['pos'].copy().astype(float)
            calibration_path = Path(cfg.urdf_path).parent.parent.parent/('arm_'+side)/'ranges.calibration.json'
            ranges = json.loads(calibration_path.read_text())
            lo = np.minimum(ranges['cal_min'], ranges['cal_max'])
            hi = np.maximum(ranges['cal_min'], ranges['cal_max'])
            if not np.isfinite(lo[:7]).all() or not np.isfinite(hi[:7]).all():
                raise RuntimeError('missing measured joint limits')
            fk[side] = lambda q, cfg=cfg: cfg.ik.fk(cfg.q2urdf(q.copy())[:7])
            target[side] = plan_wrist(start[side], fk[side], lo, hi)
        status('planned', previous_pose={s:q.tolist() for s,q in start.items()},
               target_pose={s:q.tolist() for s,q in target.items()},
               hand_axes={s:hand_axis(fk[s](q)[1]).tolist() for s,q in target.items()},
               max_wait_seconds=args.max_seconds, move_seconds=8)
        if not args.execute: return
        stop = False
        def stopping(*_):
            nonlocal stop
            stop = True
        signal.signal(signal.SIGINT, stopping)
        signal.signal(signal.SIGTERM, stopping)
        writers = {}
        command = {s:q.copy() for s,q in start.items()}
        with contextlib.ExitStack() as control:
            try:
                # Exclusive, ordinary priority only; never displace a teleop or safety writer.
                for side in configs:
                    for kind in ('ctrl', 'torque'):
                        name = f'arm_{side}.{kind}'
                        writers[name] = control.enter_context(Writer(name, Type('arm_'+kind), keeptime=False))
                def owns_control():
                    for name in writers:
                        with (Path('/dev/shm')/name).open('rb') as f:
                            f.seek(8)
                            if int.from_bytes(f.read(4), 'little') != os.getpid():
                                raise RuntimeError('control preempted; yielding without reacquiring')
                def publish(goals, enable=True, arm_max_age=.25):
                    if args.controller_pid:
                        heartbeat = json.loads((args.state/'arm-behavior.status.json').read_text())
                        if not supervisor_fresh(heartbeat, args.controller_pid, os.getppid(), time.time_ns()):
                            raise RuntimeError('search supervisor unavailable; relinquishing control')
                    owns_control()
                    states = healthy(arm_max_age)
                    for side in configs:
                        if np.max(np.abs(states[side]['pos'][:7]-command[side][:7])) > .08:
                            raise RuntimeError('arm failed to follow command; yielding to factory controller')
                        with writers[f'arm_{side}.ctrl'].buf() as b:
                            b['pos'] = goals[side]; b['vel'] = 0; b['tau'] = 0; b['alpha'] = 0
                    for side in configs:
                        with writers[f'arm_{side}.torque'].buf() as b:
                            for field in b.dtype.names:
                                if field != 'timestamp': b[field] = 0
                            b['enable'][:7] = enable  # Leave grippers unpowered, as found.
                            b['enable'][0] = True  # Preserve the factory idle mast hold.
                        command[side] = goals[side].copy()
                    return states
                # Flush a complete current-pose command before enabling torque.
                publish(start, enable=False)
                time.sleep(.02)
                status('enabling_at_previous_pose')
                # The factory driver blocks state publication for >=0.5 s while
                # verifying Operating_Mode. Hold only the saved pose for 2 s;
                # retain fresh IMU/battery checks, then require fresh arm state
                # again before allowing any trajectory movement.
                until = time.monotonic()+2
                while time.monotonic() < until:
                    publish(start, arm_max_age=2.); time.sleep(.02)
                healthy()
                def move(goals, honor_stop):
                    initial = {s:q.copy() for s,q in command.items()}
                    began = time.monotonic()
                    while True:
                        t = (time.monotonic()-began)/8.
                        publish({s:initial[s]+ease(t)*(goals[s]-initial[s]) for s in configs})
                        if t >= 1 or (honor_stop and stop): break
                        time.sleep(.02)
                status('moving_to_search')
                move(target, True)
                reason = 'cancelled' if stop else 'timeout'
                if not stop:
                    # Give the hardware filter time to settle, then measure actual finger axes.
                    until = time.monotonic()+1
                    while time.monotonic() < until:
                        publish(target); time.sleep(.02)
                    measured = {s:hand_axis(fk[s](fresh(readers[s])['pos'].astype(float))[1]) for s in configs}
                    angles = {s:float(np.degrees(np.arccos(np.clip(np.dot(v, [2**-.5,0,-2**-.5]), -1, 1)))) for s,v in measured.items()}
                    if max(angles.values()) > 7:
                        reason = 'target_not_reached'
                    else:
                        status('searching', measured_hand_axes={s:v.tolist() for s,v in measured.items()}, angular_error_deg=angles)
                        until = time.monotonic()+args.max_seconds
                        last_check = 0
                        while (args.hold_until_calibrated or time.monotonic() < until) and not stop:
                            states = publish(target)
                            if max(float(np.max(d['temp'])) for d in states.values()) >= 48:
                                reason = 'temperature_margin'; break
                            if time.monotonic()-last_check >= .25:
                                last_check = time.monotonic()
                                try:
                                    capture = capture_status()
                                    if capture['frame'] != frame:
                                        reason = 'mapping_session_changed'; break
                                    observed = scene()
                                    if calibrated(observed, args.portal, report['started_ns'], time.time_ns(), frame, clock, home_frame, baseline):
                                        reason = 'calibrated'; break
                                except (OSError, ValueError, KeyError):
                                    reason = 'mapping_unavailable'; break
                            time.sleep(.02)
                        if stop: reason = 'cancelled'
                status('restoring', reason=reason)
                move(start, False)
                until = time.monotonic()+1
                while time.monotonic() < until:
                    states = publish(start); time.sleep(.02)
                error = {s:float(np.max(np.abs(states[s]['pos'][:7]-start[s][:7]))) for s in configs}
                if max(error.values()) > .02:
                    raise RuntimeError('previous pose not reached within 7.2 degree tolerance')
                status('restored', reason=reason, return_error_turns=error)
            except Exception as exc:
                status('fault', error=str(exc), restored=False)
                raise
            # Closing ordinary writers returns control to the existing BBOS idle/safety policy.
            # Never attempt recovery motion after stale feedback or safety preemption.


if __name__ == '__main__': main()
