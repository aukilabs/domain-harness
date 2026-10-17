#!/usr/bin/env python3
"""BBOS arm joints, motor health and battery to local stdout IPC.

Reads shared memory only: no motor commands, SLAM control or peer networking.
Joints at 5 Hz from each arm's own q2urdf; motor health (temperature,
current) and the battery at 1 Hz. Each line holds one source (an arm, or the
battery) at that source's BBOS capture time, never the time it was read here.
A missing arm or battery leaves the rest running.
"""
import json
import os
import sys
import time


def main():
    from bbos import Config, Reader
    from bracketbot_capture import (arm_joint_sources, arm_joint_samples, arm_health_samples,
                                    battery_source, battery_sample)
    arms, battery = arm_joint_sources(Config, Reader), battery_source(Reader)
    if not arms and battery is None:
        raise SystemExit('No arm state or battery available')
    parent, latest = os.getppid(), {}
    offset = time.time_ns() - time.monotonic_ns()
    last_joints = last_health = 0
    # One timeline per Output: only a capture newer than the last one sent.
    sent = dict(joints=0, health=0, battery=0)

    def emit(kind, timestamp_ns, **value):
        if timestamp_ns > sent[kind]:
            sent[kind] = timestamp_ns
            print(json.dumps(dict(kind=kind, timestamp_ns=timestamp_ns, **value), allow_nan=False), flush=True)
    while os.getppid() == parent:
        if abs(time.time_ns() - time.monotonic_ns() - offset) > 250_000_000:
            raise RuntimeError('Capture clock changed; restart diagnostic')
        monotonic = time.monotonic()
        if arms and monotonic - last_joints >= .2:
            for captured, positions in arm_joint_samples(arms, latest, time.time_ns()):
                emit('joints', captured, positions=positions)
            last_joints = monotonic
        if monotonic - last_health >= 1:
            now = time.time_ns()
            for captured, joints in arm_health_samples(arms, latest, now):
                emit('health', captured, joints=joints)
            power = battery_sample(battery, latest, now)
            if power:
                emit('battery', power[0], battery=power[1])
            last_health = monotonic
        time.sleep(.02)


if __name__ == '__main__':
    sys.exit(main())
