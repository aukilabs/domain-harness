import base64
import hashlib
import importlib.util
import json
from pathlib import Path
import socket
import sys
import tempfile
import threading
import unittest
import uuid

import numpy as np

scripts = Path(__file__).resolve().parents[1] / 'scripts'
sys.path.insert(0, str(scripts))
spec = importlib.util.spec_from_file_location('diagnostic_nav', scripts / 'diagnostic-nav.py')
nav = importlib.util.module_from_spec(spec)
spec.loader.exec_module(nav)


def go(**changes):
    value = dict(op='go', request_id=str(uuid.uuid4()), waypoints=[[1, 2, None], [3, 4, 0.5]])
    value.update(changes)
    return json.dumps(value)


class FakeNav:
    """A one-client WebSocket server: answers the handshake, sends one text frame, records one."""

    def __init__(self, outgoing):
        self.server = socket.socket()
        self.server.bind(('127.0.0.1', 0))
        self.server.listen(1)
        self.address = self.server.getsockname()
        self.received = []
        self.thread = threading.Thread(target=self.run, args=(outgoing,), daemon=True)
        self.thread.start()

    def run(self, outgoing):
        client, _ = self.server.accept()
        reader = client.makefile('rb')
        headers = {}
        while (line := reader.readline()) not in (b'\r\n', b''):
            if b':' in line:
                k, v = line.decode().split(':', 1)
                headers[k.strip().lower()] = v.strip()
        accept = base64.b64encode(hashlib.sha1((headers['sec-websocket-key'] +
                 '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
        client.sendall(f'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
                       f'Sec-WebSocket-Accept: {accept}\r\n\r\n'.encode())
        payload = json.dumps(outgoing).encode()
        client.sendall(bytes([0x81, len(payload)]) + payload)
        first, second = reader.read(2)
        assert second & 0x80, 'client frames must be masked'
        mask = reader.read(4)
        data = bytes(b ^ mask[i % 4] for i, b in enumerate(reader.read(second & 0x7F)))
        self.received.append(json.loads(data))
        client.close()


class DiagnosticNavTests(unittest.TestCase):
    def test_requests_are_strict(self):
        op, request_id, waypoints = nav.parse(go())
        self.assertEqual(op, 'go')
        self.assertEqual(waypoints, [(1., 2., None), (3., 4., .5)])
        self.assertEqual(nav.parse(json.dumps(dict(op='stop', request_id=request_id)))[0], 'stop')
        for bad in (go(op='drive'), go(request_id='x'), go(waypoints=[]), go(waypoints=[[1, 2]]),
                    go(waypoints=[[float('nan'), 0, None]]), go(waypoints=[[0, 0, 'north']]), go(extra=1),
                    go(waypoints=[[0, 0, None]] * 65)):
            with self.assertRaises((ValueError, KeyError)):
                nav.parse(bad)

    def test_messages_match_the_runners_goto_path(self):
        messages = nav.nav_messages([(1., 2., None), (3., 4., .5)])
        self.assertEqual([m['type'] for m in messages], ['clear', 'add_wp', 'add_wp', 'start'])
        self.assertNotIn('h', messages[1])
        self.assertEqual(messages[2]['h'], .5)

    def test_outcome_reports_progress_arrival_and_failure(self):
        outcome = nav.Outcome()
        # A completed patrol from before this goal started is not an arrival.
        self.assertEqual([p for p, _ in outcome.update(dict(status='patrol complete', running=False))], ['progress'])
        self.assertEqual([p for p, _ in outcome.update(dict(status='driving', running=True))], ['progress'])
        self.assertEqual(outcome.update(dict(status='driving', running=True)), [])
        phases = outcome.update(dict(status='patrol complete', running=False, rx=1, ry=2, rh=0))
        self.assertEqual([p for p, _ in phases], ['progress', 'arrived'])
        self.assertEqual(phases[1][1]['nav_pose'], [1, 2, 0])
        failed = nav.Outcome().update(dict(status='patrol failed — skipped WP 2', running=False))
        self.assertEqual([p for p, _ in failed], ['progress', 'failed'])

    def test_nav_frame_follows_navs_own_rule(self):
        with tempfile.TemporaryDirectory() as d:
            maps = Path(d)
            pose, mode, files = nav.nav_from_world(maps)
            self.assertEqual((mode, pose['rotation_xyzw'], files), ('mapless', [0, 0, 0, 1], {}))
            matrix = np.eye(4)
            matrix[:3, :3] = [[0, -1, 0], [1, 0, 0], [0, 0, 1]]
            matrix[:3, 3] = [1, 2, 0]
            np.save(maps / 'T_nav_gl.npy', matrix)
            # A transform without QR relocalization is not what nav uses.
            self.assertEqual(nav.nav_from_world(maps)[1], 'mapless')
            (maps / 'portals_colmap.json').write_text('{}')
            pose, mode, files = nav.nav_from_world(maps)
            self.assertEqual(mode, 'map')
            self.assertEqual(pose['translation'], [1, 2, 0])
            np.testing.assert_allclose(np.abs(pose['rotation_xyzw']), [0, 0, np.sqrt(.5), np.sqrt(.5)], atol=1e-9)
            self.assertEqual(len(files), 2)

    def test_websocket_client_handshakes_reads_and_sends_masked_frames(self):
        server = FakeNav(dict(status='idle', ready=True))
        ws = nav.WebSocket(server.address)
        self.assertEqual(json.loads(ws.receive()), dict(status='idle', ready=True))
        ws.send(dict(type='stop'))
        server.thread.join(3)
        ws.close()
        self.assertEqual(server.received, [dict(type='stop')])


if __name__ == '__main__':
    unittest.main()
