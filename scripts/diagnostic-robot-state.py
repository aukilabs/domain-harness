#!/usr/bin/env python3
"""BBOS arm joints, motor health and battery to local stdout IPC.

Reads shared memory only: no motor commands, SLAM control or peer networking.
Joints at 5 Hz from each arm's own q2urdf; health (temperature, current,
battery) at 1 Hz. A missing arm or battery leaves the rest running.
"""
import json
import os
import sys
import time


def main():
    from bbos import Config, Reader
    from bracketbot_capture import (arm_joint_sources, arm_joint_positions, arm_joint_health,
                                    battery_source, battery_reading)
    arms, battery = arm_joint_sources(Config, Reader), battery_source(Reader)
    if not arms and battery is None:
        raise SystemExit('No arm state or battery available')
    parent, latest = os.getppid(), {}
    offset = time.time_ns() - time.monotonic_ns()
    last_joints = last_health = previous = 0
    emit = lambda value: print(json.dumps(value, allow_nan=False), flush=True)
    while os.getppid() == parent:
        if abs(time.time_ns() - time.monotonic_ns() - offset) > 250_000_000:
            raise RuntimeError('Capture clock changed; restart diagnostic')
        monotonic = time.monotonic()
        if arms and monotonic - last_joints >= .2:
            joints = arm_joint_positions(arms, latest, time.time_ns())
            # One timeline per Output: skip a repeat of the newest arm sample.
            if joints and joints[0] > previous:
                previous = joints[0]
                emit(dict(kind='joints', timestamp_ns=joints[0], positions=joints[1]))
            last_joints = monotonic
        if monotonic - last_health >= 1:
            now = time.time_ns()
            joint_health, power = arm_joint_health(arms, latest, now), battery_reading(battery, latest, now)
            if joint_health or power:
                emit(dict(kind='health', timestamp_ns=now, joints=joint_health, battery=power))
            last_health = monotonic
        time.sleep(.02)


if __name__ == '__main__':
    sys.exit(main())
