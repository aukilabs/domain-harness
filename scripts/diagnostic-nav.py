#!/usr/bin/env python3
"""navigate adapter for the diagnostic peer: hands waypoints to BBOS
navigation and reports its progress. Local IPC only. Two BBOS generations:

- Newer BBOS runs navigation as the always-on `nav` daemon: goals go on the
  `nav.command` topic (held open while a route runs: the daemon stops if its
  writer disconnects) and progress comes from `nav.state`. The daemon reads
  `slam.pose` directly, so its frame is the SLAM world (identity).
- Older BBOS runs the nav app: goals go on ws://127.0.0.1:8010/ws, and its
  frame is `T_nav_gl.npy` applied to the SLAM world when a nav map is installed
  with QR relocalization on, identity when it runs mapless.

The first stdout line declares that relationship and the backend in use.

Then each stdin line is one request:
  {"op": "go", "request_id": UUID, "waypoints": [[x, y, h|null], ...]}   nav frame
  {"op": "stop", "request_id": UUID}
and stdout answers {"request_id", "phase": ...}: "accepted" or "rejected" for a
go, then "progress" on each nav status change, then "arrived" or "failed";
"stopped" for a stop. Nav itself plans, avoids obstacles and writes drive.ctrl.
"""
import base64
import json
import math
import os
from pathlib import Path
import socket
import sys
import threading
import time
import uuid

NAV = ('127.0.0.1', 8010)
MAPS = Path.home() / 'bbapps/nav/maps'
READY_WAIT_S = 3.


def nav_from_world(maps=MAPS):
    """(pose, mode, files): nav's own rule for T_nav_gl, with the files it used."""
    transform, colmap = maps / 'T_nav_gl.npy', maps / 'portals_colmap.json'
    if not (transform.is_file() and colmap.is_file()):
        return dict(translation=[0., 0., 0.], rotation_xyzw=[0., 0., 0., 1.]), 'mapless', {}
    import numpy as np
    from bracketbot_capture import rotation_quaternion
    matrix = np.load(transform)
    if matrix.shape != (4, 4) or not np.isfinite(matrix).all():
        raise ValueError('invalid T_nav_gl.npy')
    files = {str(p): p.stat().st_mtime_ns for p in (transform, colmap)}
    return (dict(translation=matrix[:3, 3].tolist(), rotation_xyzw=rotation_quaternion(matrix[:3, :3])),
            'map', files)


def parse(line):
    request = json.loads(line)
    if not isinstance(request, dict) or request.get('op') not in ('go', 'stop'):
        raise ValueError('op must be go or stop')
    request_id = str(uuid.UUID(str(request['request_id'])))
    if request['op'] == 'stop':
        if set(request) != {'op', 'request_id'}:
            raise ValueError('stop takes only request_id')
        return 'stop', request_id, None
    if set(request) != {'op', 'request_id', 'waypoints'}:
        raise ValueError('go takes request_id and waypoints')
    points = request['waypoints']
    if not isinstance(points, list) or not 1 <= len(points) <= 64:
        raise ValueError('1..64 waypoints')
    waypoints = []
    for point in points:
        if not isinstance(point, list) or len(point) != 3:
            raise ValueError('waypoint is [x, y, h|null]')
        x, y, h = point
        if not all(isinstance(v, (int, float)) and math.isfinite(v) and abs(v) < 1e4 for v in (x, y)):
            raise ValueError('invalid waypoint')
        if h is not None and not (isinstance(h, (int, float)) and math.isfinite(h)):
            raise ValueError('invalid heading')
        waypoints.append((float(x), float(y), None if h is None else float(h)))
    return 'go', request_id, waypoints


def nav_messages(waypoints):
    """What the runner's nav_ws.goto_path sends: clear, add_wp..., start."""
    out = [dict(type='clear')]
    for x, y, h in waypoints:
        out.append(dict(type='add_wp', x=x, y=y) if h is None else dict(type='add_wp', x=x, y=y, h=h))
    out.append(dict(type='start'))
    return out


class Outcome:
    """Turns nav state frames into this request's progress, arrival or failure."""

    def __init__(self):
        self.started = False
        self.last = None

    def update(self, state):
        status, running = str(state.get('status', '')), bool(state.get('running'))
        events = []
        if running:
            self.started = True
        if status != self.last:
            self.last = status
            events.append(('progress', dict(status=status, running=running)))
        if status.startswith('patrol failed'):
            events.append(('failed', dict(reason=status)))
        elif self.started and not running and status == 'patrol complete':
            events.append(('arrived', dict(nav_pose=[state.get('rx'), state.get('ry'), state.get('rh')])))
        return events


class DaemonOutcome:
    """nav.state (idle, waiting_for_drive, navigating, reached, failed) to progress."""

    def __init__(self):
        self.started = False
        self.last = None

    def update(self, state, reason):
        events = []
        if state == 'navigating':
            self.started = True
        if (state, reason) != self.last:
            self.last = (state, reason)
            events.append(('progress', dict(status=state, reason=reason)))
        if state == 'failed':
            events.append(('failed', dict(reason=reason or 'nav failed')))
        elif self.started and state == 'reached':
            events.append(('arrived', {}))
        elif self.started and state == 'idle':
            events.append(('failed', dict(reason=reason or 'navigation stopped')))
        return events


def daemon_record(buffer, waypoints, enabled=True):
    """Fill a nav_command record: up to 128 [x, y, heading|NaN] waypoints."""
    import numpy as np
    buffer['enabled'] = enabled
    buffer['num_waypoints'] = len(waypoints) if enabled else 0
    buffer['waypoints'][:] = 0
    for i, (x, y, h) in enumerate(waypoints if enabled else []):
        buffer['waypoints'][i] = (x, y, np.nan if h is None else h)
    buffer['loop'] = False
    buffer['global_goal'] = False


class DaemonNav:
    """Newer BBOS: the nav daemon over nav.command / nav.state."""
    mode = 'daemon'

    def __init__(self):
        self.writer = None
        self.pose = dict(translation=[0., 0., 0.], rotation_xyzw=[0., 0., 0., 1.])
        self.files = {}

    def command(self):
        # Opened with the first goal and kept: the daemon stops a route whose writer goes away.
        if self.writer is None:
            from bbos import Type, Writer
            try:
                writer = Writer('nav.command', Type('nav_command'), keeptime=False)
                writer.__enter__()
            except RuntimeError as e:
                raise ValueError(f'nav.command is held by another program: {e}')
            self.writer = writer
        return self.writer

    def start(self, on_state):
        from bbos import Reader

        def follow():
            with Reader('nav.state', keeptime=False) as reader:
                while True:
                    if reader.ready():
                        d = reader.data
                        on_state(bytes(d['state']).decode(errors='replace').strip('\0'),
                                 bytes(d['reason']).decode(errors='replace').strip('\0'))
                    time.sleep(.1)
        threading.Thread(target=follow, daemon=True).start()

    def ready(self):
        return True

    def go(self, waypoints):
        with self.command().buf() as buffer:
            daemon_record(buffer, waypoints)

    def stop(self):
        with self.command().buf() as buffer:
            daemon_record(buffer, [], enabled=False)


def daemon_available():
    try:
        from bbos import Type
        Type('nav_command')
        return True
    except Exception:
        return False


class WebSocket:
    """Minimal RFC 6455 text client: enough for nav's local /ws."""

    def __init__(self, address, path='/ws', timeout=READY_WAIT_S):
        self.sock = socket.create_connection(address, timeout=timeout)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall((f'GET {path} HTTP/1.1\r\nHost: {address[0]}:{address[1]}\r\nUpgrade: websocket\r\n'
                           f'Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n').encode())
        self.reader = self.sock.makefile('rb')
        status = self.reader.readline()
        if b' 101 ' not in status:
            raise ConnectionError(f'nav refused the WebSocket: {status!r}')
        while self.reader.readline() not in (b'\r\n', b''):
            pass
        self.lock = threading.Lock()

    def _frame(self, opcode, payload):
        mask = os.urandom(4)
        length = len(payload)
        header = bytes([0x80 | opcode])
        if length < 126:
            header += bytes([0x80 | length])
        elif length < 65536:
            header += bytes([0x80 | 126]) + length.to_bytes(2, 'big')
        else:
            header += bytes([0x80 | 127]) + length.to_bytes(8, 'big')
        with self.lock:
            self.sock.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))

    def send(self, message):
        self._frame(0x1, json.dumps(message).encode())

    def receive(self):
        parts = []
        while True:
            first, second = self.reader.read(2)
            opcode, length = first & 0x0F, second & 0x7F
            if length == 126:
                length = int.from_bytes(self.reader.read(2), 'big')
            elif length == 127:
                length = int.from_bytes(self.reader.read(8), 'big')
            if second & 0x80:
                mask = self.reader.read(4)
                payload = bytes(b ^ mask[i % 4] for i, b in enumerate(self.reader.read(length)))
            else:
                payload = self.reader.read(length)
            if opcode == 0x8:
                raise ConnectionError('nav closed the WebSocket')
            if opcode == 0x9:
                self._frame(0xA, payload)
                continue
            if opcode in (0x1, 0x0):
                parts.append(payload)
                if first & 0x80:
                    return b''.join(parts).decode()

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


class AppNav:
    """Older BBOS: the nav app on ws://127.0.0.1:8010/ws."""
    mode = 'app'

    def __init__(self):
        self.pose, self.map_mode, self.files = nav_from_world()
        self.mode = 'app-' + self.map_mode
        self.ws = None
        self.connected = threading.Event()

    def start(self, on_state):
        def follow():
            # One long-lived connection: state frames for progress, and the channel commands go out on.
            while True:
                try:
                    ws = WebSocket(NAV)
                    ws.sock.settimeout(None)
                    self.ws = ws
                    while True:
                        frame = json.loads(ws.receive())
                        if frame.get('t') == 'params' or 'status' not in frame:
                            continue
                        if frame.get('ready'):
                            self.connected.set()
                        on_state(frame, None)
                except (OSError, ValueError, ConnectionError):
                    self.connected.clear()
                    self.ws = None
                    on_state(None, 'lost the nav app connection')
                    time.sleep(1)
        threading.Thread(target=follow, daemon=True).start()

    def ready(self):
        if self.files and any(Path(p).stat().st_mtime_ns != t for p, t in self.files.items()):
            raise ValueError('nav map changed since start; restart the diagnostic')
        return self.connected.wait(READY_WAIT_S) and self.ws is not None

    def go(self, waypoints):
        for message in nav_messages(waypoints):
            self.ws.send(message)
            time.sleep(.02)

    def stop(self):
        self.ws.send(dict(type='stop'))


def main():
    out_lock = threading.Lock()

    def emit(request_id, phase, **details):
        with out_lock:
            print(json.dumps(dict(request_id=request_id, phase=phase, **details), allow_nan=False), flush=True)

    nav = DaemonNav() if daemon_available() else AppNav()
    with out_lock:
        print(json.dumps(dict(nav_from_world=nav.pose, mode=nav.mode), allow_nan=False), flush=True)
    current = {'id': None, 'outcome': None}
    current_lock = threading.Lock()

    def handle_state(state, reason):
        request_id, outcome = current['id'], current['outcome']
        if not (request_id and outcome):
            return
        if state is None:
            events = [('failed', dict(reason=reason))]
        elif isinstance(nav, DaemonNav):
            events = outcome.update(state, reason)
        else:
            events = outcome.update(state)
        for phase, details in events:
            emit(request_id, phase, **details)
            if phase in ('arrived', 'failed'):
                current['id'] = current['outcome'] = None

    def on_state(state, reason):
        with current_lock:
            handle_state(state, reason)

    nav.start(on_state)
    parent = os.getppid()
    for line in sys.stdin:
        if os.getppid() != parent:
            break
        request_id = None
        try:
            if len(line) > 16384:
                raise ValueError('request too large')
            op, request_id, waypoints = parse(line)
            if not nav.ready():
                raise ValueError('nav app is not running or has no slam.pose yet (:8010)')
            with current_lock:
                if op == 'stop':
                    nav.stop()
                    if current['id']:
                        emit(current['id'], 'failed', reason='cancelled')
                    current['id'] = current['outcome'] = None
                    emit(request_id, 'stopped')
                    continue
                # The daemon atomically stops and clears its old plan when a new
                # command record arrives. The app backend clears before starting.
                # Do not leave the superseded request waiting for an outcome.
                nav.go(waypoints)
                if current['id']:
                    emit(current['id'], 'failed', reason='replaced by a new navigation command')
                current['id'] = request_id
                current['outcome'] = DaemonOutcome() if isinstance(nav, DaemonNav) else Outcome()
                emit(request_id, 'accepted', mode=nav.mode, waypoints=len(waypoints))
        except (ValueError, KeyError, OSError, ConnectionError, RuntimeError) as e:
            emit(request_id, 'rejected', reason=str(e))


if __name__ == '__main__':
    main()
