"""Shared ordinary-priority BBOS arm control with live feedback and ownership."""
import contextlib
import json
import os
from pathlib import Path
import time
import numpy as np
from bracketbot_capture import timestamp
from bracketbot_behavior import supervisor_fresh


class Arms:
    def __init__(self, state, controller_pid):
        from bbos import Config,Reader
        self.state,self.controller_pid=state,controller_pid
        self.stack=contextlib.ExitStack();self.writers={}
        self.configs={s:Config('arm_'+s) for s in ('left','right')}
        self.readers={s:self.reader(Reader,'arm_'+s+'.state') for s in self.configs}
        self.imu=self.reader(Reader,'imu.orientation');self.battery=self.reader(Reader,'drive.status')
        self.low_voltage=Config('base').low_battery_v
        self.limits={}
        for s,cfg in self.configs.items():
            cfg.ik.init()
            path=Path(cfg.urdf_path).parent.parent.parent/('arm_'+s)/'ranges.calibration.json'
            ranges=json.loads(path.read_text());lo=np.minimum(ranges['cal_min'],ranges['cal_max']);hi=np.maximum(ranges['cal_min'],ranges['cal_max'])
            if not np.isfinite([lo,hi]).all():raise RuntimeError('missing measured limits')
            self.limits[s]=(lo,hi)

    def reader(self, Reader, name):
        r=Reader(name,keeptime=False);r.__enter__()
        # BBOS __exit__ suppresses exceptions; callbacks preserve failures.
        self.stack.callback(r.__exit__,None,None,None)
        return r

    def close(self):self.stack.close()

    def fresh(self,reader,limit=.25):
        reader.ready();d=reader.data.copy()
        if not 0 <= (time.time_ns()-timestamp(d))/1e9 <= limit:raise RuntimeError('stale arm/safety feedback')
        return d

    def healthy(self,arm_age=.25):
        if np.max(np.abs(self.fresh(self.imu)['rpy'][:2]))>10:raise RuntimeError('robot tilted')
        if float(self.fresh(self.battery,15)['voltage'])<self.low_voltage:raise RuntimeError('low battery')
        states={s:self.fresh(r,arm_age) for s,r in self.readers.items()}
        for s,d in states.items():
            if not all(np.isfinite(d[f]).all() for f in ('pos','vel','temp','current')):raise RuntimeError('nonfinite feedback')
            if np.max(d['temp'])>=48 or np.any(np.abs(d['current'])>=np.array(self.configs[s].hard_current_limit)*.8):
                raise RuntimeError('arm protection margin')
        return states

    def acquire(self,states):
        from bbos import Writer,Type
        if any(np.max(np.abs(d['vel']))>.02 for d in states.values()):raise RuntimeError('arm already moving')
        self.command={s:d['pos'].astype(float).copy() for s,d in states.items()}
        for s in self.configs:
            for kind in ('ctrl','torque'):
                name=f'arm_{s}.{kind}';w=Writer(name,Type('arm_'+kind),keeptime=False)
                self.stack.callback(w.__exit__,None,None,None);self.writers[name]=w

    def publish(self,goals,enable=True,arm_age=.25):
        heartbeat=json.loads((self.state/'arm-behavior.status.json').read_text())
        if not supervisor_fresh(heartbeat,self.controller_pid,os.getppid(),time.time_ns()):raise RuntimeError('controller unavailable')
        for name in self.writers:
            with (Path('/dev/shm')/name).open('rb') as f:
                f.seek(8)
                if int.from_bytes(f.read(4),'little')!=os.getpid():raise RuntimeError('manual/safety control preempted reach')
        states=self.healthy(arm_age)
        for s,d in states.items():
            lo,hi=self.limits[s];goal=np.asarray(goals[s])
            if (not np.isfinite(goal).all() or np.any(goal[:7]<lo[:7]) or np.any(goal[:7]>hi[:7])
                    or np.max(np.abs(d['pos'][:7]-self.command[s][:7]))>.035):raise RuntimeError('joint bounds/following error')
            with self.writers[f'arm_{s}.ctrl'].buf() as b:
                b['pos']=goal;b['vel']=0;b['tau']=0;b['alpha']=0
        for s in states:
            with self.writers[f'arm_{s}.torque'].buf() as b:
                for f in b.dtype.names:
                    if f!='timestamp':b[f]=0
                b['enable'][:7]=enable;b['enable'][0]=True
        self.command={s:np.array(q).copy() for s,q in goals.items()}
        return states
