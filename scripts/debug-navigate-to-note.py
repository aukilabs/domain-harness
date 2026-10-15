#!/usr/bin/env python3
"""Debug: send a robot running the diagnostic peer with --navigate to a note.

Reads the note's position from Home's local scene (HTTP on this Mac), then
hands the goal to the robot's local navigate-debug inbox over SSH. The robot
runs the same path as the navigate Operable: it resolves the scene point
through its own published transforms into the BBOS nav frame and gives it to
the nav app, which plans, avoids obstacles and drives. No Home change needed.

  python3 scripts/debug-navigate-to-note.py --robot 0181 --note robot-target
  python3 scripts/debug-navigate-to-note.py --robot 0181 --note robot-target --start-nav
  python3 scripts/debug-navigate-to-note.py --robot 0181 --stop
"""
import argparse
import json
import shlex
import subprocess
import sys
import time
import urllib.request
import uuid

ROOT = '~/diagnostics/connection-peer'
MANAGER = '/home/bracketbot/bbos/bbos/daemons/manager/result/bin'


def ssh(robot, command, stdin=None, timeout=60):
    result = subprocess.run(['bb', '--llm', 'ssh', robot, command], input=stdin, capture_output=True,
                            text=True, timeout=timeout)
    return result.returncode, result.stdout


def find_note(home, scene, name):
    with urllib.request.urlopen(f'{home}/api/local-scenes/{scene}', timeout=10) as response:
        view = json.load(response)
    notes = view.get('notes') or []
    title = lambda n: ((n.get('details') or {}).get('title') or n.get('text') or '').strip()
    matches = [n for n in notes if title(n).lower() == name.lower()]
    if len(matches) != 1:
        listed = ', '.join(repr(title(n)) for n in notes) or 'none'
        sys.exit(f'{"No" if not matches else "Several"} note(s) named {name!r} in {scene}. Notes: {listed}')
    return view['frame'], matches[0]


def nav_up(robot):
    """Newer BBOS: the always-on nav daemon. Older BBOS: the nav app on :8010."""
    code, out = ssh(robot, "(pgrep -f 'daemon.py nav' >/dev/null || ss -ltn | grep -qE ':8010\\b') && echo up || echo down")
    return out.strip().endswith('up')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--robot', required=True)
    ap.add_argument('--note', default='robot-target')
    ap.add_argument('--scene', default='humanoid-hub-conference')
    ap.add_argument('--home', default='http://127.0.0.1:4319')
    ap.add_argument('--start-nav', action='store_true', help="start the BBOS nav app if it isn't running")
    ap.add_argument('--stop', action='store_true', help='stop the current navigation instead')
    ap.add_argument('--follow', type=float, default=60, help='seconds to follow nav progress (0 = none)')
    args = ap.parse_args()

    request = dict(request_id=str(uuid.uuid4()))
    if args.stop:
        request['op'] = 'stop'
    else:
        frame, note = find_note(args.home, args.scene, args.note)
        request.update(frame_id=frame, point=note['position'])
        print(f'Note {args.note!r} at {[round(v, 2) for v in note["position"]]} m in {frame}')

    if not nav_up(args.robot):
        if not args.start_nav:
            sys.exit('Neither the BBOS nav daemon nor the nav app (:8010) is running. Rerun with --start-nav to start the app.')
        print('Starting the BBOS nav app...')
        ssh(args.robot, f'{MANAGER}/run nav')
        until = time.time() + 45
        while not nav_up(args.robot):
            if time.time() > until:
                sys.exit('nav did not start listening on :8010 within 45 s')
            time.sleep(3)
    print('BBOS navigation is running')

    inbox = f'{ROOT}/navigate-debug'
    code, _ = ssh(args.robot, f'mkdir -p {inbox} && rm -f {inbox}/response.json && '
                              f'cat > {inbox}/request.tmp && mv {inbox}/request.tmp {inbox}/request.json',
                  stdin=json.dumps(request))
    if code:
        sys.exit('Could not hand the request to the robot')
    until = time.time() + 20
    while True:
        code, out = ssh(args.robot, f'cat {inbox}/response.json 2>/dev/null')
        if out.strip():
            break
        if time.time() > until:
            sys.exit('No answer from the robot. Is the diagnostic peer running with --navigate?')
        time.sleep(1)
    response = json.loads(out)
    if not response.get('ok'):
        sys.exit('Refused: ' + response.get('error', 'unknown'))
    result = response['result']
    if args.stop:
        print('Stopped.')
        return
    print(f'Accepted: {len(result["waypoints_nav"])} waypoint(s) in {result["nav_frame"]["id"]}: '
          f'{[[round(v, 2) for v in p] for p in result["waypoints_nav"]]}, '
          f'final heading {round(result["final_heading_rad"] * 57.29578)}°')
    if args.follow <= 0:
        return
    print(f'Following nav for up to {args.follow:.0f} s (Ctrl-C to stop watching; the robot keeps going)...')
    log = f'{ROOT}/diagnostic.log'
    watch = (f'timeout {int(args.follow)} tail -n 0 -F {log} | grep -a --line-buffered '
             + shlex.quote(request['request_id']))
    process = subprocess.Popen(['bb', '--llm', 'ssh', args.robot, watch], stdout=subprocess.PIPE, text=True)
    try:
        for line in process.stdout:
            try:
                event = json.loads(line.split('Nav adapter: ', 1)[1])
            except (IndexError, ValueError):
                continue
            details = {k: v for k, v in event.items() if k not in ('request_id', 'phase')}
            print(f'  {event["phase"]}: {details}')
            if event['phase'] in ('arrived', 'failed'):
                break
    except KeyboardInterrupt:
        pass
    finally:
        process.terminate()


if __name__ == '__main__':
    main()
