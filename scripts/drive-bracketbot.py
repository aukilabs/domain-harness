#!/usr/bin/env python3
"""Drive the Bracketbot base from one allowed phone's joystick.

Reads the runner's cached drive command and writes `drive.ctrl`. Without
`--execute` it only reports what it would do. It holds the drive writer only
while a fresh command exists, so manual teleop is never locked out while the
phone is idle, and it refuses to start if another program is driving.

Stops come from several independent places: the phone sends stops on release,
Home withholds stale commands, this controller ignores input older than one second,
and the base daemon itself halts 100 ms after the last `drive.ctrl` write.
"""
import argparse
import json
from pathlib import Path
import signal
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bracketbot_drive import STOP, twist  # noqa: E402

HZ = 20
RELEASE_AFTER_S = 1.0


def read_packet(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return {}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--state', type=Path, required=True, help='runner state directory')
    parser.add_argument('--driver-peer', required=True, help='peer ID of the only phone allowed to drive')
    parser.add_argument('--execute', action='store_true', help='actually write drive.ctrl')
    parser.add_argument('--fast', action='store_true', help='full teleop speeds instead of half')
    args = parser.parse_args()
    scale = 1.0 if args.fast else 0.5
    stop = False

    def halt(*_):
        nonlocal stop
        stop = True
    signal.signal(signal.SIGINT, halt)
    signal.signal(signal.SIGTERM, halt)

    import numpy as np
    from bbos import Config, Reader, Type, Writer
    half_width = float(Config('drive').robot_width) * 0.5
    low_voltage = float(Config('base').low_battery_v)
    status_path = args.state / 'phone-drive.json'
    command_path = args.state / 'drive-command.json'
    writer = None
    last_motion = 0.0
    last_status = 0.0
    last_reason = None

    def write(target, value):
        with target.buf() as buffer:
            buffer['twist'] = np.array(value, dtype=np.float32)
            buffer['twist_torque'] = np.zeros(2, dtype=np.float32)

    def release():
        nonlocal writer
        if writer is not None:
            try:
                write(writer, STOP)
            finally:
                writer.__exit__(None, None, None)
                writer = None

    battery = Reader('drive.status', keeptime=False)
    battery.__enter__()
    try:
        while not stop:
            began = time.monotonic()
            value, reason = twist(read_packet(command_path), allowed_peer=args.driver_peer,
                                  now_ns=time.time_ns(), half_width_m=half_width, scale=scale)
            if battery.ready() and float(battery.data['voltage']) < low_voltage:
                value, reason = STOP, 'low battery'
            if reason is None and args.execute:
                if writer is None:
                    try:
                        writer = Writer('drive.ctrl', Type('drive_ctrl'), keeptime=False, buf_ms=500)
                        writer.__enter__()
                    except RuntimeError as error:  # another program is driving
                        writer, value, reason = None, STOP, f'drive.ctrl busy: {error}'
                if writer is not None:
                    write(writer, value)
                    last_motion = began
            elif writer is not None:
                # Hold an explicit zero briefly, then let go of the topic.
                write(writer, STOP)
                if began - last_motion > RELEASE_AFTER_S:
                    release()
            if reason != last_reason or began - last_status > 0.5:
                last_reason, last_status = reason, began
                report = dict(schema='spatial.robot-phone-drive/v1', updated_ns=time.time_ns(),
                              executing=bool(args.execute), driving=reason is None and args.execute,
                              twist=list(value), reason=reason, scale=scale,
                              allowed_driver_peer_id=args.driver_peer)
                temporary = status_path.with_suffix('.tmp')
                temporary.write_text(json.dumps(report))
                temporary.replace(status_path)
            time.sleep(max(0.0, 1 / HZ - (time.monotonic() - began)))
    finally:
        release()
        battery.__exit__(None, None, None)


if __name__ == '__main__':
    main()
