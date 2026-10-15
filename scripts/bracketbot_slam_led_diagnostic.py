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
from pathlib import Path
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time


def status(health, received, now):
    if health is None or now - received > 3:
        return 'unknown (no fresh BBOS health)'
    if bool(health['vo_lost']):
        return 'lost'
    if bool(health['stalled']):
        return 'stalled'
    return 'tracking' if bool(health['localized']) else 'unlocalized'


# A refused or stopped point_at request: blink for a few seconds and say why.
ALERT_SECONDS = 6
ALERT_COLORS = {'unreachable': [255, 140, 0]}
ALERT_SPEECH = {
    'hot': "Sorry, I can't point right now. My arm motor is too hot.",
    'current': 'Sorry, I stopped pointing. My arm was straining too hard.',
    'unreachable': "Sorry, I can't point there.",
}


def alert_color(code, since):
    """Blinking alert colour, or None once the alert is over."""
    if since > ALERT_SECONDS:
        return None
    on = int(since*4) % 2 == 0
    return ALERT_COLORS.get(code, [255, 0, 0]) if on else [0, 0, 0]


def speak(code):
    """Say why pointing stopped, through BBOS's speaker; silent without espeak-ng."""
    backend = shutil.which('espeak-ng') or shutil.which('espeak')
    if backend is None:
        return None
    here = Path(__file__).resolve().parent

    def run():
        with tempfile.TemporaryDirectory(prefix='bracketbot-alert-') as directory:
            wave = str(Path(directory)/'alert.wav')
            text = ALERT_SPEECH.get(code, "Sorry, I couldn't point.")
            # Text is stdin, never a shell command or option.
            if subprocess.run([backend, '--stdin', '-w', wave], input=text.encode(), timeout=30,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
                subprocess.run([sys.executable, str(here/'bracketbot-python.py'), '--daemon', 'speaker',
                                str(here/'play-bracketbot-speech.py'), wave], timeout=60,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    thread = threading.Thread(target=run, daemon=True)
    thread.start()
    return thread


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


# A peer that exits (a BBOS SLAM restart needs a fresh session; a crashed helper
# needs respawning) is started again after a growing delay, reset by a long run.
RESTART_DELAYS_S = (2, 4, 8, 16, 32, 60)
STEADY_RUN_S = 60


def restart_delay(failures):
    return RESTART_DELAYS_S[min(failures, len(RESTART_DELAYS_S)-1)]


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
        alert = None
        pose_reader = None
        last_pose_sent = 0
        last_pose_write = 0
        peer_started = 0.0
        restart_at = None
        failures = 0

        def start_peer():
            started = subprocess.Popen(args.peer_command, stdout=subprocess.PIPE,
                                       stdin=subprocess.PIPE if args.publish_poses else subprocess.DEVNULL)
            os.set_blocking(started.stdout.fileno(), False)
            if args.publish_poses:
                os.set_blocking(started.stdin.fileno(), False)
            return started

        def close_peer(old):
            for pipe in (old.stdin, old.stdout):
                if pipe is not None:
                    try:
                        pipe.close()
                    except OSError:
                        pass
        try:
            if args.peer_command:
                peer = start_peer()
                peer_started = time.monotonic()
                if args.publish_poses:
                    pose_reader = Reader('slam.pose', keeptime=False)
                    pose_reader.__enter__()
            with Reader('slam.health', keeptime=False) as reader:
                while not stopped:
                    now = time.monotonic()
                    if peer is not None and restart_at is None and peer.poll() is not None:
                        failures = 0 if now - peer_started >= STEADY_RUN_S else failures + 1
                        delay = restart_delay(failures)
                        print(time.strftime('%Y-%m-%dT%H:%M:%S%z'), f'peer exited ({peer.returncode}); restarting in {delay} s', flush=True)
                        close_peer(peer)
                        restart_at = now + delay
                    if restart_at is not None and now >= restart_at:
                        peer = start_peer()
                        peer_started, restart_at, pending, last_pose_sent = now, None, b'', 0
                        print(time.strftime('%Y-%m-%dT%H:%M:%S%z'), 'peer restarted', flush=True)
                    if peer is not None and restart_at is None:
                        if select.select([peer.stdout], [], [], 0)[0]:
                            chunk = os.read(peer.stdout.fileno(), 4096)
                            pending += chunk
                            while b'\n' in pending:
                                line, pending = pending.split(b'\n', 1)
                                if line in (b'connected', b'disconnected'):
                                    connected = line == b'connected'
                                    connection_received = now
                                elif line.startswith(b'alert '):
                                    code = line[6:].decode(errors='replace')[:32]
                                    print(time.strftime('%Y-%m-%dT%H:%M:%S%z'), 'pointing alert', code, flush=True)
                                    alert = (code, now)
                                    speak(code)
                            if len(pending) > 4096:
                                raise RuntimeError('Invalid peer status output')
                        if peer.poll() is not None or now - connection_received > 3:
                            connected = False
                    elif peer is not None:
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
                        if sample is not None and restart_at is None and peer.poll() is None:
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
                    if alert is not None:
                        flash = alert_color(alert[0], now-alert[1])
                        if flash is None:
                            alert = None
                        else:
                            color = flash
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
                if peer.stdin is not None and not peer.stdin.closed:
                    peer.stdin.close()
                if peer.poll() is None:
                    peer.terminate()
                    try:
                        peer.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        peer.kill()
                        peer.wait()
                if not peer.stdout.closed:
                    peer.stdout.close()


if __name__ == '__main__':
    main()
