#!/usr/bin/env python3
"""BBOS health -> red LED; optional minimal peer -> green, with red priority.

Without --peer-command this remains the independent LED-only diagnostic.
Optional --publish-poses reads BBOS poses for the SDK peer.
No camera, map, or motion access. Does not start automatically on boot.
"""
import fcntl
import argparse
import json
import math
import os
import select
import signal
import subprocess
import time


def status(health, received, now):
    if health is None or now - received > 3:
        return 'unknown (no fresh BBOS health)'
    if bool(health['vo_lost']):
        return 'lost'
    if bool(health['stalled']):
        return 'stalled'
    return 'tracking' if bool(health['localized']) else 'unlocalized'


def led_color(tracking, connected):
    if tracking in ('lost', 'stalled'):
        return [255, 0, 0]
    if tracking == 'tracking' and connected:
        return [0, 255, 0]
    return None


def pose_sample(record, tracking, now_ns, source_id):
    timestamp = int(record['timestamp'].astype('int64'))
    if tracking != 'tracking' or not 0 <= now_ns - timestamp <= 800_000_000:
        return None
    position = [float(v) for v in record['pos']]
    quaternion = [float(v) for v in record['quat']]
    if len(position) != 3 or len(quaternion) != 4 or not all(math.isfinite(v) for v in position + quaternion):
        return None
    norm = math.sqrt(sum(v*v for v in quaternion))
    if abs(norm-1) > .001:
        return None
    return dict(timestamp_ns=timestamp, source_id=source_id,
                pose=dict(translation=position, rotation_xyzw=[v/norm for v in quaternion]))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--publish-poses', action='store_true')
    parser.add_argument('--peer-command', nargs=argparse.REMAINDER,
                        help='Optional connection-only peer; stdout is its local liveness pipe')
    args = parser.parse_args()
    if args.publish_poses and not args.peer_command:
        parser.error('--publish-poses requires --peer-command')
    from bbos import Reader, Writer, Type
    stopped = False

    def stop(*_):
        nonlocal stopped
        stopped = True

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)
    with open('/tmp/bbos-slam-led-diagnostic.lock', 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        writer = None
        health = previous = timestamp = None
        received = last_sent = 0
        peer = None
        connected = False
        connection_received = 0
        pending = b''
        previous_color = None
        pose_reader = None
        last_pose_sent = 0
        last_pose_write = 0
        try:
            if args.peer_command:
                peer = subprocess.Popen(args.peer_command, stdout=subprocess.PIPE,
                                        stdin=subprocess.PIPE if args.publish_poses else subprocess.DEVNULL)
                os.set_blocking(peer.stdout.fileno(), False)
                if args.publish_poses:
                    os.set_blocking(peer.stdin.fileno(), False)
                    pose_reader = Reader('slam.pose', keeptime=False)
                    pose_reader.__enter__()
            with Reader('slam.health', keeptime=False) as reader:
                while not stopped:
                    now = time.monotonic()
                    if peer is not None:
                        if select.select([peer.stdout], [], [], 0)[0]:
                            chunk = os.read(peer.stdout.fileno(), 4096)
                            pending += chunk
                            while b'\n' in pending:
                                line, pending = pending.split(b'\n', 1)
                                if line in (b'connected', b'disconnected'):
                                    connected = line == b'connected'
                                    connection_received = now
                            if len(pending) > 4096:
                                raise RuntimeError('Invalid peer status output')
                        if peer.poll() is not None or now - connection_received > 3:
                            connected = False
                    if reader.ready():
                        sample = reader.data.copy()
                        stamp = int(sample['timestamp'].astype('int64'))
                        # Do not mistake repeated reads of a retained sample for updates.
                        if stamp != timestamp and -100_000_000 <= time.time_ns() - stamp <= 3_000_000_000:
                            timestamp, health, received = stamp, sample, now
                    current = status(health, received, now)
                    if pose_reader is not None:
                        sample = None
                        if pose_reader.ready():
                            sample = pose_sample(pose_reader.data.copy(), current, time.time_ns(), pose_reader._writer_pid)
                            if sample is not None and sample['timestamp_ns'] <= last_pose_sent:
                                sample = None
                        if sample is None and (current != 'tracking' or now-last_pose_write > .8) and now-last_pose_write > .2:
                            sample = dict(timestamp_ns=time.time_ns(), pose=None, source_id=None)
                        if sample is not None and peer.poll() is None:
                            try:
                                # Small atomic pipe writes; never block BBOS polling on networking.
                                os.write(peer.stdin.fileno(), (json.dumps(sample)+'\n').encode())
                                last_pose_sent = sample['timestamp_ns']
                                last_pose_write = now
                            except (BlockingIOError, BrokenPipeError):
                                pass
                    state = (current, connected)
                    if state != previous:
                        print(time.strftime('%Y-%m-%dT%H:%M:%S%z'), current,
                              'Home connected' if connected else 'Home disconnected', flush=True)
                        previous = state
                    color = led_color(current, connected)
                    if color is not None:
                        if writer is None:
                            writer = Writer('led.ctrl', Type('led_ctrl'))
                            writer.__enter__()
                            last_sent = 0
                        if color != previous_color or now - last_sent >= 1:
                            with writer.buf() as buf:
                                buf['rgb'] = color
                                buf['brightness'] = -1
                                buf['period_ms'] = 0
                                for name, value in [('animation', b''), ('pixels', 0), ('charging_level', 0)]:
                                    if name in buf.dtype.names:
                                        buf[name] = value
                            last_sent = now
                    elif writer is not None:
                        writer.__exit__(None, None, None)
                        writer = None
                    previous_color = color
                    time.sleep(.01 if args.publish_poses else .1)
        finally:
            if pose_reader is not None:
                pose_reader.__exit__(None, None, None)
            if writer is not None:
                writer.__exit__(None, None, None)
            if peer is not None:
                if peer.stdin is not None:
                    peer.stdin.close()
                if peer.poll() is None:
                    peer.terminate()
                    try:
                        peer.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        peer.kill()
                        peer.wait()
                peer.stdout.close()


if __name__ == '__main__':
    main()
