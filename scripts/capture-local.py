#!/usr/bin/env python3
"""Read-only sensor acquisition for a portal-origin map; no Home/backend poses."""
import base64
import importlib.util
import json
import multiprocessing as mp
import os
from pathlib import Path
import queue
import signal
import time


def helper():
    spec=importlib.util.spec_from_file_location('rgbd_capture',Path(__file__).with_name('capture-rgbd.py'))
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    return module


def camera(output,stop):
    from galbot_sdk.g1 import GalbotRobot,SensorType
    h=helper();signal.signal(signal.SIGTERM,h.terminate)
    parent=os.getppid();robot=GalbotRobot()
    a,b=SensorType.RIGHT_ARM_CAMERA,SensorType.RIGHT_ARM_DEPTH_CAMERA
    output.cancel_join_thread()
    try:
        if not robot.init({a,b},True):raise RuntimeError('camera initialization failed')
        time.sleep(2);info,provenance=h.calibration(robot,a,b);last_frame=0
        while not stop.is_set() and os.getppid()==parent:
            started=time.monotonic()
            try:
                sample=h.capture(robot,a,b,info,provenance,True)
                if sample['timestamp_ns']<=last_frame:
                    stop.wait(.1);continue
                last_frame=sample['timestamp_ns'];value={'kind':'camera','sample':sample}
            except Exception as e:value={'kind':'status','detail':'Camera: '+str(e)}
            try:output.put_nowait(value)
            except queue.Full:pass
            stop.wait(max(.01,1-(time.monotonic()-started)))
    finally:robot.request_shutdown();robot.destroy()


def emit(value):
    print('SENSOR '+json.dumps(value,separators=(',',':'),allow_nan=False),flush=True)


def main():
    from galbot_sdk.g1 import GalbotRobot,SensorType
    h=helper();signal.signal(signal.SIGTERM,h.terminate)
    parent=os.getppid();ctx=mp.get_context('spawn');output=ctx.Queue(1);stop=ctx.Event()
    worker=ctx.Process(target=camera,args=(output,stop));worker.start()
    robot=GalbotRobot()
    try:
        if not robot.init({SensorType.BASE_LIDAR}):raise RuntimeError('LiDAR initialization failed')
        last_odom=0;last_lidar=0;last_scan=0
        while os.getppid()==parent:
            started=time.monotonic()
            sample=robot.get_odom()
            if sample and sample['timestamp_ns']>last_odom:
                last_odom=sample['timestamp_ns'];q=sample['orientation'];norm=sum(v*v for v in q)**.5
                emit(dict(kind='odometry',timestamp_ns=last_odom,pose=dict(translation=sample['position'],rotation_xyzw=[v/norm for v in q])))
            elif sample and sample['timestamp_ns']<last_odom:
                raise RuntimeError('odometry clock reset; new local map session required')
            try:emit(output.get_nowait())
            except queue.Empty:pass
            if started-last_lidar>=1:
                last_lidar=started;data=robot.get_lidar_data(SensorType.BASE_LIDAR)
                if data and data['header']['timestamp_ns']>last_scan:
                    if data['header']['frame_id']!='lidar_base_link':raise RuntimeError('unexpected LiDAR frame')
                    last_scan=data['header']['timestamp_ns']
                    pose,stamp=robot.get_transform('base_link','lidar_base_link',last_scan)
                    if len(pose)==7 and stamp==last_scan:
                        cloud={k:data[k] for k in ['width','height','point_step','row_step','is_bigendian','fields']}
                        cloud.update(kind='point_cloud',data_base64=base64.b64encode(data['data']).decode())
                        emit(dict(kind='lidar',timestamp_ns=last_scan,reading=dict(sensor_id='lidar.base',measurement=cloud),
                                  sensor_to_base=dict(translation=pose[:3],rotation_xyzw=pose[3:])))
            time.sleep(max(.001,.05-(time.monotonic()-started)))
    finally:
        stop.set();worker.join(3)
        if worker.is_alive():worker.terminate();worker.join(2)
        robot.request_shutdown();robot.destroy()

if __name__=='__main__':main()
