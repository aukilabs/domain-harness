#!/usr/bin/env python3
"""navigate adapter for the diagnostic peer: hands waypoints to the BBOS nav
app (ws://127.0.0.1:8010/ws) and reports its progress. Local IPC only.

The first stdout line declares where the nav frame sits relative to the frame
`slam.pose` is in, exactly as nav decides it at start: `T_nav_gl.npy` when a
nav map is installed with QR relocalization on, identity when nav runs mapless.

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


def main():
    out_lock = threading.Lock()

    def emit(request_id, phase, **details):
        with out_lock:
            print(json.dumps(dict(request_id=request_id, phase=phase, **details), allow_nan=False), flush=True)

    pose, mode, files = nav_from_world()
    with out_lock:
        print(json.dumps(dict(nav_from_world=pose, mode=mode), allow_nan=False), flush=True)
    current = {'id': None, 'outcome': None}
    socket_holder = {'ws': None, 'ready': threading.Event()}

    def follow():
        # One long-lived connection: state frames for progress, and the channel commands go out on.
        while True:
            try:
                ws = WebSocket(NAV)
                ws.sock.settimeout(None)
                socket_holder['ws'] = ws
                while True:
                    frame = json.loads(ws.receive())
                    if frame.get('t') == 'params' or 'status' not in frame:
                        continue
                    if frame.get('ready'):
                        socket_holder['ready'].set()
                    request_id, outcome = current['id'], current['outcome']
                    if request_id and outcome:
                        for phase, details in outcome.update(frame):
                            emit(request_id, phase, **details)
                            if phase in ('arrived', 'failed'):
                                current['id'] = current['outcome'] = None
            except (OSError, ValueError, ConnectionError):
                socket_holder['ready'].clear()
                socket_holder['ws'] = None
                if current['id']:
                    emit(current['id'], 'failed', reason='lost the nav app connection')
                    current['id'] = current['outcome'] = None
                time.sleep(1)

    threading.Thread(target=follow, daemon=True).start()
    parent = os.getppid()
    for line in sys.stdin:
        if os.getppid() != parent:
            break
        request_id = None
        try:
            if len(line) > 16384:
                raise ValueError('request too large')
            op, request_id, waypoints = parse(line)
            if files and any(Path(p).stat().st_mtime_ns != t for p, t in files.items()):
                raise ValueError('nav map changed since start; restart the diagnostic')
            if not socket_holder['ready'].wait(READY_WAIT_S) or socket_holder['ws'] is None:
                raise ValueError('nav app is not running or has no slam.pose yet (:8010)')
            ws = socket_holder['ws']
            if op == 'stop':
                ws.send(dict(type='stop'))
                if current['id']:
                    emit(current['id'], 'failed', reason='cancelled')
                current['id'] = current['outcome'] = None
                emit(request_id, 'stopped')
                continue
            current['id'], current['outcome'] = request_id, Outcome()
            for message in nav_messages(waypoints):
                ws.send(message)
                time.sleep(.02)
            emit(request_id, 'accepted', mode=mode, waypoints=len(waypoints))
        except (ValueError, KeyError, OSError, ConnectionError) as e:
            emit(request_id, 'rejected', reason=str(e))


if __name__ == '__main__':
    main()
