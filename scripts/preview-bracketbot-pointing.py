#!/usr/bin/env python3
"""One read-only selection/IK preview. No control writers, no execution option.

Run through bracketbot-python.py --daemon arm_left. Stdin is a diagnostic
Home presence snapshot: {presence, age_ms}, where age_ms includes transport
delay measured by the caller. Not an authenticated motion-control input.
"""
import argparse
import contextlib
import json
from pathlib import Path
import sys
import time
import urllib.request
import numpy as np
from bracketbot_capture import timestamp
from bracketbot_pointing import select_target
from bracketbot_pointing_plan import base_from_arm, candidate


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--state', type=Path, required=True)
    ap.add_argument('--ik', action='store_true', help='Compute an unexecuted candidate for the selected arm')
    ap.add_argument('--presence-url', help='Diagnostic-only Home HTTP presence endpoint; never a control channel')
    args = ap.parse_args()
    from bbos import Config, Reader
    configs = {side: Config('arm_'+side) for side in ('left', 'right')}
    transforms = {}
    for side, cfg in configs.items():
        cfg.ik.init()
        transforms[side] = base_from_arm(cfg.urdf_path)
    began = time.monotonic()
    if args.presence_url:
        with urllib.request.urlopen(args.presence_url, timeout=2) as response:
            packet = {'presence': json.load(response), 'age_ms': 0}
    else:
        packet = json.load(sys.stdin)
    # Motion helpers consume the native-time local adapter, not SDK datasets
    # whose capture timestamps are expressed in the runner's session clock.
    scene_path = args.state/'robot-control.json'
    scene_stat = scene_path.stat()
    scene = json.loads(scene_path.read_text())
    home = json.loads((args.state/'home-connection.json').read_text())
    hands, states, ages = {}, {}, []
    with contextlib.ExitStack() as stack:
        for side, cfg in configs.items():
            reader = stack.enter_context(Reader('arm_'+side+'.state', keeptime=False))
            reader.ready()
            state = reader.data.copy()
            states[side] = state['pos'].astype(float)
            ages.append((time.time_ns()-timestamp(state))/1e6)
            position, _ = cfg.ik.fk(cfg.q2urdf(states[side].copy())[:7])
            hands[side] = (transforms[side] @ np.r_[position, 1])[:3].tolist()
    now = time.time_ns()
    robot_pose = scene['registries']['local_mapping'].get('robot_pose')
    input_ages = dict(scene_age_ms=(now-scene_stat.st_mtime_ns)/1e6,
        presence_age_ms=packet['age_ms']+(time.monotonic()-began)*1000,
        arm_age_ms=(max(ages)+(time.monotonic()-began)*1000
                    if all(np.isfinite(age) and age >= 0 for age in ages) else float('inf')),
        robot_pose_age_ms=(now-robot_pose['capture_timestamp_ns'])/1e6 if robot_pose else float('inf'))
    intent = select_target(scene, packet['presence'], hands, **input_ages,
                           expected_home_frame=home['frame'])
    result = {'intent': intent, 'diagnostic_only': True, 'actuation_enabled': False,
              'input_ages_ms': {k: v if np.isfinite(v) else None for k, v in input_ages.items()}}
    if args.ik and intent['status'] == 'target_selected':
        side = intent['arm']; cfg = configs[side]
        target = (np.linalg.inv(transforms[side]) @ np.r_[intent['phone_position_m'], 1])[:3]
        ranges_path = Path(cfg.urdf_path).parent.parent.parent/('arm_'+side)/'ranges.calibration.json'
        ranges = json.loads(ranges_path.read_text())
        lower = np.minimum(ranges['cal_min'], ranges['cal_max'])
        upper = np.maximum(ranges['cal_min'], ranges['cal_max'])
        try:
            result['plan'] = candidate(cfg, states[side], target, lower, upper)
        except (ValueError, RuntimeError) as exc:
            result['plan'] = {'status': 'rejected', 'reason': str(exc), 'actuation_enabled': False}
    print(json.dumps(result, allow_nan=False))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, RuntimeError) as exc:
        print(json.dumps({'status': 'blocked', 'reason': str(exc), 'actuation_enabled': False}))
        sys.exit(1)
