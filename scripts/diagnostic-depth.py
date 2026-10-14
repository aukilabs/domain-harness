#!/usr/bin/env python3
"""BBOS measured points to local stdout IPC. No SLAM control or peer networking."""
import json
import os
import select
import struct
import sys
import time


def capture(record, transform):
    from bracketbot_capture import optical_points, sampled_points
    count = int(record['num_points'])
    if not 0 <= count <= len(record['points']) or count > len(record['colors']):
        raise ValueError('Invalid BBOS point count')
    stamp = int(record['timestamp'].astype('int64'))
    points, colors = sampled_points(optical_points(record['points'][:count], transform),
                                    record['colors'][:count], stamp, max_points=2048)
    return dict(timestamp_ns=stamp, points=points, colors=colors)


def encode_capture(sample):
    """Bounded local IPC, distinct from the registry-backed SDK wire envelope."""
    import numpy as np
    points = np.asarray(sample['points'], dtype='<f4')
    colors = np.asarray(sample['colors'], dtype=np.uint8)
    if (points.ndim != 2 or points.shape[1] != 3 or not 1 <= len(points) <= 2048
            or colors.shape != points.shape or not np.isfinite(points).all()):
        raise ValueError('Invalid depth IPC capture')
    return (struct.pack('<QIB', sample['timestamp_ns'], len(points), 1)
            + points.tobytes() + colors.tobytes())


def main():
    import numpy as np
    from bbos import Config, Reader
    from bracketbot_capture import rotation_quaternion
    config = Config('depth')
    if getattr(config, 'imu_pitch', 0):
        raise ValueError('Dynamic depth mount requires capture-matched extrinsics')
    transform = np.asarray(config.camera_to_base_3x4, dtype=float).reshape(3, 4)
    if not np.isfinite(transform).all():
        raise ValueError('Invalid depth extrinsic')
    print(json.dumps(dict(sensor_to_base=dict(translation=transform[:, 3].tolist(),
        rotation_xyzw=rotation_quaternion(transform[:, :3]))), allow_nan=False), flush=True)
    parent, previous, writer = os.getppid(), 0, None
    wanted = False
    offset = time.time_ns() - time.monotonic_ns()
    with Reader('camera.points', keeptime=False) as reader:
        while os.getppid() == parent:
            # Local stdin control from the SDK subscription gate, never a network API.
            if select.select([sys.stdin], [], [], 0)[0]:
                command = os.read(sys.stdin.fileno(), 4096)
                if not command:
                    break
                commands = command.splitlines()
                if not commands or any(c not in (b'0', b'1') for c in commands):
                    raise ValueError('Invalid depth demand control')
                wanted = commands[-1] == b'1'
            if abs(time.time_ns() - time.monotonic_ns() - offset) > 250_000_000:
                raise RuntimeError('Capture clock changed; restart diagnostic')
            if wanted and reader.ready():
                record = reader.data.copy()
                stamp = int(record['timestamp'].astype('int64'))
                source = reader._writer_pid
                if writer is not None and writer != source:
                    raise RuntimeError('Depth writer changed; restart diagnostic')
                writer = source
                if stamp < previous:
                    raise RuntimeError('Depth timestamp regressed; restart diagnostic')
                if stamp > previous and 0 <= time.time_ns()-stamp < 800_000_000:
                    previous = stamp
                    sample = capture(record, transform)
                    if sample['points']:
                        sys.stdout.buffer.write(encode_capture(sample))
                        sys.stdout.buffer.flush()
            time.sleep(.2)


if __name__ == '__main__':
    main()
