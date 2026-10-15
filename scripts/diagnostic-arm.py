#!/usr/bin/env python3
"""point_at arm adapter for the diagnostic peer. Local stdin/stdout IPC only.

The first stdout line declares where the arm IK frame (arm_base: X forward,
Y left, Z up, origin on the drive axle) sits in BBOS base (X right, Y forward,
Z up). Then each stdin line is one request:

  {"request_id": UUID, "arm": "left"|"right", "target_arm_base": [x, y, z]}

and stdout answers {"request_id", "phase": "planned"|"rejected", ...}. Only
with --execute does a planned request then move the arm: "moving", "holding",
"returning", then "done" or "fault". Without --execute no arm writer is opened.

A move takes the arm's writers (never displacing another writer), enables at
the current pose, eases to sides HOME, swings the straight arm forward, aims,
holds, eases back to HOME and closes the writers so BBOS's idle policy takes
over again. SIGTERM during a move or hold returns to HOME first. Stop
quest_teleop / mimic / view_arms --control first. No base or gripper commands.
"""
import argparse
import contextlib
import json
import math
import os
from pathlib import Path
import signal
import sys
import time
import uuid

import numpy as np

RATE_HZ = 50
# set_arm_pose.py's default move time; the mast still sets its own pace and
# a following error beyond 0.08 turns stops the move.
MOVE_SECONDS = 2.5
AIM_SECONDS = 1.5
HOLD_SECONDS = 3.
# The factory driver stops publishing state for >=0.5 s while enabling; hold
# at least this long, then until state is fresh again, at most the maximum.
ENABLE_SETTLE_SECONDS = .5
ENABLE_SETTLE_MAX_SECONDS = 2.
REST_SETTLE_SECONDS = .5
# BBOS trips a motor after 15 s above 50 C; a move heats the mast lift, so a
# request is refused at plan time with this much headroom left.
TEMP_LIMIT_C = 50.
PLAN_TEMP_MARGIN_C = 3.
# Assumed, as in bracketbot_aim: BBOS base's origin is on the floor under the
# drive axle. arm_base is BBOS base turned +90 deg about Z, raised to the axle.
ARM_BASE_IN_BASE = dict(translation=[0., 0., .165/2],
                        rotation_xyzw=[0., 0., math.sqrt(.5), math.sqrt(.5)])


def parse_request(line):
    request = json.loads(line)
    if not isinstance(request, dict) or set(request) != {'request_id', 'arm', 'target_arm_base'}:
        raise ValueError('request must have exactly request_id, arm and target_arm_base')
    request_id = str(uuid.UUID(str(request['request_id'])))
    if request['arm'] not in ('left', 'right'):
        raise ValueError('arm must be left or right')
    target = np.asarray(request['target_arm_base'], float)
    if target.shape != (3,) or not np.isfinite(target).all():
        raise ValueError('target must be a finite arm_base point')
    return request_id, 'arm_'+request['arm'], target


def kinematics(cfg):
    def fk(q):
        pos, quat = cfg.ik.fk(list(cfg.q2urdf(np.asarray(q, float).copy())[:7]))
        return np.asarray(pos, float), np.asarray(quat, float)
    return fk


def measured_limits(cfg, name):
    path = Path(cfg.urdf_path).parent.parent.parent/name/'ranges.calibration.json'
    if not path.exists():
        raise RuntimeError(f'{name} is not range-calibrated; run its calibrate.py first')
    ranges = json.loads(path.read_text())
    lower = np.minimum(ranges['cal_min'], ranges['cal_max'])
    upper = np.maximum(ranges['cal_min'], ranges['cal_max'])
    if not np.isfinite(lower[:7]).all() or not np.isfinite(upper[:7]).all():
        raise RuntimeError(f'{name} has no finite measured limits')
    return lower, upper


class ArmProtection(RuntimeError):
    """A protection margin, with a short code (hot, current) for alerts."""

    def __init__(self, code, reason):
        super().__init__(reason)
        self.code = code


class Arms:
    """Per-arm kinematics, limits and live feedback readers."""

    def __init__(self, stack):
        from bbos import Config, Reader
        self.arms = {}
        for name in ('arm_left', 'arm_right'):
            cfg = Config(name)
            cfg.ik.init()
            self.arms[name] = dict(cfg=cfg, fk=kinematics(cfg), limits=measured_limits(cfg, name),
                                   state=stack.enter_context(Reader(name+'.state', keeptime=False)))
        self.imu = stack.enter_context(Reader('imu.orientation', keeptime=False))
        self.battery = stack.enter_context(Reader('drive.status', keeptime=False))
        self.low_voltage = Config('base').low_battery_v

    def fresh(self, reader, max_age=.25, wait=3.):
        from bracketbot_capture import timestamp
        # ready() polls for a newer sample; data keeps the latest one. Slow
        # feeds (drive.status, ~10 s) are judged by age.
        until = time.monotonic()+wait
        reader.ready()
        while reader.data is None:
            if time.monotonic() > until:
                raise RuntimeError(f'no data on {reader._name} (is its BBOS daemon running?)')
            time.sleep(.01)
            reader.ready()
        d = reader.data.copy()
        if not -.1 <= (time.time_ns()-timestamp(d))/1e9 <= max_age:
            raise RuntimeError('stale BBOS feedback; relinquishing to factory controller')
        return d

    def healthy(self, name, arm_max_age=.25, temp_limit=TEMP_LIMIT_C):
        if np.max(np.abs(self.fresh(self.imu)['rpy'][:2])) > 10:
            raise RuntimeError('robot tilted; yielding to factory safety controller')
        if float(self.fresh(self.battery, 15)['voltage']) < self.low_voltage:
            raise RuntimeError('battery below factory low-voltage threshold')
        arm = self.arms[name]
        d = self.fresh(arm['state'], arm_max_age)
        for field in ('pos', 'vel', 'temp', 'current'):
            if not np.isfinite(d[field]).all():
                raise RuntimeError('invalid arm feedback')
        joints = list(arm['cfg'].joint_names)
        hot = int(np.argmax(d['temp']))
        if d['temp'][hot] >= temp_limit:
            raise ArmProtection('hot', f"{joints[hot]} at {d['temp'][hot]:.0f} C (limit {temp_limit:.0f} C)")
        limits = np.asarray(arm['cfg'].hard_current_limit, float)*.8
        over = np.flatnonzero(np.abs(d['current']) >= limits)
        if over.size:
            i = int(over[0])
            raise ArmProtection('current', f"{joints[i]} drawing {abs(d['current'][i]):.1f} A (limit {limits[i]:.1f} A)")
        return d


def plan(arms, name, target):
    """Checked joint targets for one request; raises ValueError to refuse it."""
    from bracketbot_point import HOME, check_path, plan_point_at
    arm = arms.arms[name]
    fk, (lower, upper) = arm['fk'], arm['limits']
    state = arms.healthy(name, temp_limit=TEMP_LIMIT_C-PLAN_TEMP_MARGIN_C)
    if np.max(np.abs(state['vel'])) > .02:
        raise ValueError('arm is moving')
    start = state['pos'].astype(float).copy()
    home = HOME[name].copy()
    home[7] = start[7]  # the gripper stays as found
    check_path(fk, start, home, name)
    forward, aimed, report = plan_point_at(fk, home, lower, upper, name, target)
    forward[7] = aimed[7] = start[7]
    return start, home, forward, aimed, report


def execute(arms, name, poses, stopped, emit):
    """Move through HOME, forward and aimed, hold, and return. Raises on a fault."""
    from bbos import Type, Writer
    from bracketbot_capture import timestamp
    start, home, forward, aimed = poses
    with contextlib.ExitStack() as stack:
        # Exclusive, ordinary priority only; a held topic raises instead of waiting.
        writers = {kind: stack.enter_context(Writer(f'{name}.{kind}', Type('arm_'+kind), keeptime=False))
                   for kind in ('ctrl', 'torque')}
        command = start.copy()

        def publish(goal, enable=True, arm_max_age=.25):
            for kind in writers:
                with (Path('/dev/shm')/f'{name}.{kind}').open('rb') as f:
                    f.seek(8)
                    if int.from_bytes(f.read(4), 'little') != os.getpid():
                        raise RuntimeError('control preempted; yielding without reacquiring')
            d = arms.healthy(name, arm_max_age)
            if enable and np.max(np.abs(d['pos'][:7]-command[:7])) > .08:
                raise RuntimeError('arm failed to follow command; yielding to factory controller')
            with writers['ctrl'].buf() as b:
                b['pos'] = goal; b['vel'] = 0; b['tau'] = 0; b['alpha'] = 0
            with writers['torque'].buf() as b:
                for field in b.dtype.names:
                    if field != 'timestamp': b[field] = 0
                b['enable'][:7] = enable
                b['enable'][0] = True  # preserve the factory idle mast hold
            command[:] = goal
            return d

        def move(goal, seconds, honor_stop):
            from bracketbot_point import smootherstep
            initial = command.copy()
            # Mast travel sets the pace (arm-control.md: 2.5 s per 0.35 turns).
            seconds = max(seconds, 2.5*abs(goal[0]-initial[0])/.35)
            began = time.monotonic()
            while True:
                t = (time.monotonic()-began)/seconds
                publish(initial+smootherstep(t)*(goal-initial))
                if t >= 1 or honor_stop and stopped():
                    return not (honor_stop and stopped())
                time.sleep(1/RATE_HZ)

        def hold(goal, seconds, honor_stop=True):
            until = time.monotonic()+seconds
            while time.monotonic() < until and not (honor_stop and stopped()):
                publish(goal); time.sleep(1/RATE_HZ)

        # Flush the current pose, then enable at it. The factory driver stops
        # publishing state for >=0.5 s while switching modes; hold through it.
        publish(start, enable=False)
        time.sleep(.02)
        began = time.monotonic()
        while True:
            d = publish(start, arm_max_age=2.)
            settled = time.monotonic()-began
            fresh = (time.time_ns()-timestamp(d))/1e9 <= .1
            if settled >= ENABLE_SETTLE_MAX_SECONDS or settled >= ENABLE_SETTLE_SECONDS and fresh:
                break
            time.sleep(1/RATE_HZ)
        emit('moving')
        if move(home, MOVE_SECONDS, True) and move(forward, MOVE_SECONDS, True) and move(aimed, AIM_SECONDS, True):
            d = publish(aimed)
            emit('holding', measured_hand_m=arms.arms[name]['fk'](d['pos'].astype(float))[0].tolist())
            hold(aimed, HOLD_SECONDS)
        emit('returning')
        move(forward, AIM_SECONDS, False)
        move(home, MOVE_SECONDS, False)
        hold(home, REST_SETTLE_SECONDS, False)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--execute', action='store_true')
    args = ap.parse_args()
    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append(True))
    signal.signal(signal.SIGINT, lambda *_: stop.append(True))
    print(json.dumps(dict(arm_base_to_base=ARM_BASE_IN_BASE), allow_nan=False), flush=True)
    parent = os.getppid()
    with contextlib.ExitStack() as stack:
        arms = Arms(stack)
        for line in sys.stdin:
            if stop or os.getppid() != parent:
                break
            if len(line) > 4096:
                raise ValueError('arm request too large')
            request_id = None

            def emit(phase, **details):
                print(json.dumps(dict(request_id=request_id, phase=phase, **details), allow_nan=False), flush=True)
            try:
                request_id, name, target = parse_request(line)
                *poses, report = plan(arms, name, target)
            except (ValueError, RuntimeError) as e:
                code = getattr(e, 'code', 'unreachable' if isinstance(e, ValueError) else 'other')
                emit('rejected', reason=str(e), code=code)
                continue
            emit('planned', arm=name, executes=args.execute, aim_error_deg=report['aim_error_deg'],
                 hand_floor_m=report['aimed_hand_floor_m'])
            if not args.execute:
                continue
            try:
                execute(arms, name, poses, lambda: bool(stop), emit)
                emit('done')
            except Exception as e:  # The writers are closed; BBOS idle policy holds the arm.
                emit('fault', reason=str(e), code=getattr(e, 'code', 'other'))
                raise


if __name__ == '__main__':
    main()
