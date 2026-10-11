#!/usr/bin/env python3
"""Isolated numerical worker in the existing arm runtime. Reads feedback only."""
import contextlib
import json
import os
import signal
import time
import numpy as np
from bbos import Config, Reader
from bracketbot_capture import timestamp
from bracketbot_occupancy import Model, spec


def main():
    output = os.fdopen(os.dup(1), 'w', buffering=1)
    os.dup2(2, 1)
    context = json.loads(os.environ['SPATIAL_CAPTURE_CONTEXT'])
    configs = {side: Config('arm_'+side) for side in ('left', 'right')}
    model = Model(configs['left'].urdf_path)
    def emit(value): output.write('SENSOR '+json.dumps(value, allow_nan=False, separators=(',', ':'))+'\n')
    emit(dict(kind='register_sensor', spec=spec(context, model)))
    stopped = False
    def stop(*_):
        nonlocal stopped
        stopped = True
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    parent = os.getppid()
    with contextlib.ExitStack() as stack:
        readers = {}
        for side in configs:
            reader = Reader('arm_'+side+'.state', keeptime=False)
            reader.__enter__(); stack.callback(reader.__exit__, None, None, None)
            readers[side] = reader
        while not stopped and os.getppid() == parent:
            start = time.monotonic(); now = time.time_ns(); positions = {}
            for side, reader in readers.items():
                if not reader.ready(): continue
                record = reader.data.copy()
                if not -100_000_000 <= now-timestamp(record) <= 250_000_000: continue
                q = configs[side].q2urdf(record['pos'].copy().astype(float))
                if len(q) != len(configs[side].joint_names) or not np.isfinite(q).all(): continue
                positions.update(zip(configs[side].joint_names, q))
            payload = model.sample(positions, context['session_id'])
            emit(dict(kind='sensor_sample', sensor_id='robot.occupancy', timestamp_ns=now,
                      data=json.dumps(payload, allow_nan=False), bytes=''))
            time.sleep(max(0, .5-(time.monotonic()-start)))


if __name__ == '__main__': main()
