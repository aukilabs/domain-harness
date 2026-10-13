#!/usr/bin/env python3
"""Read one existing BBOS raw eye. Local stdout IPC only; no controls or network."""
import base64
import json
import os
import time


def main():
    import cv2
    import numpy as np
    from bbos import Config, Reader
    from bracketbot_capture import rotation_quaternion, fresh_jpeg
    cv2.setNumThreads(1)
    cfg = Config('depth')
    if getattr(cfg, 'imu_pitch', 0):
        raise ValueError('Dynamic camera mount requires timestamped extrinsics')
    kl, dl, _, _, rl, *_ = cfg.camera_cal()
    width, height = int(cfg.input_width)//2, int(cfg.input_height)
    rectified_to_base = np.asarray(cfg.camera_to_base_3x4, dtype=float).reshape(3, 4)
    # R1 maps raw optical rays to rectified optical rays. Compose into base.
    rotation = rectified_to_base[:, :3] @ np.asarray(rl).reshape(3, 3)
    calibration = dict(width=width, height=height,
                       k=[float(kl[0, 0]), float(kl[1, 1]), float(kl[0, 2]), float(kl[1, 2])],
                       d=np.asarray(dl).reshape(-1)[:4].tolist(),
                       camera_to_base=dict(translation=rectified_to_base[:, 3].tolist(),
                                           rotation_xyzw=rotation_quaternion(rotation)))
    print(json.dumps(calibration, allow_nan=False), flush=True)
    parent, previous, writer = os.getppid(), 0, None
    wall_offset = time.time_ns() - time.monotonic_ns()
    with Reader('camera.head.jpeg', keeptime=False) as reader:
        while os.getppid() == parent:
            if abs(time.time_ns() - time.monotonic_ns() - wall_offset) > 250_000_000:
                raise RuntimeError('Capture clock changed; restart diagnostic for a fresh session')
            if reader.ready():
                sample = reader.data.copy()
                stamp = int(sample['timestamp'].astype('int64'))
                source = reader._writer_pid
                if writer is not None and source != writer:
                    raise RuntimeError('Camera writer changed; restart diagnostic')
                writer = source
                if stamp < previous:
                    raise RuntimeError('Camera timestamp regressed; restart diagnostic')
                if stamp > previous and 0 <= time.time_ns()-stamp < 800_000_000:
                    previous = stamp
                    # Same installed BBOS JPEG record layout as the main capture adapter.
                    raw = fresh_jpeg(sample, time.time_ns())
                    if raw is None:
                        continue
                    image = cv2.imdecode(np.frombuffer(raw, np.uint8), cv2.IMREAD_GRAYSCALE)
                    if image is None or image.shape != (height, width*2):
                        raise ValueError('Camera geometry changed')
                    ok, jpeg = cv2.imencode('.jpg', image[:, :width], [cv2.IMWRITE_JPEG_QUALITY, 95])
                    if not ok:
                        raise ValueError('JPEG encoding failed')
                    print(json.dumps(dict(timestamp_ns=stamp, jpeg=base64.b64encode(jpeg).decode())), flush=True)
            time.sleep(.2)


if __name__ == '__main__':
    main()
