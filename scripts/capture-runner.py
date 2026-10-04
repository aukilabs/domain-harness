#!/usr/bin/env python3
"""One runner's read-only acquisition. Helpers have no Auki peer or identity."""
from collections import deque
import base64
import json
import os
import signal
import sys
import threading
import time
from pathlib import Path
from types import SimpleNamespace

# Use the reviewed adapters alongside this entry point.
sys.path.insert(0,str(Path(__file__).parent))
sys.path.insert(0,str(Path(__file__).parent.parent / 'runtime'))

OUTPUT=None

class Publisher:
    def __init__(self):
        self.lock=threading.Lock();self.specs=[];self.samples={};self.lidar=None;self.pose_sources=set();self.poses=deque(maxlen=2048)
    def register_sensor(self,raw):
        spec=json.loads(raw)
        with self.lock:
            self.specs.append(dict(kind='register_sensor',spec=spec))
            if spec['payload']['datatype']=='auki.odometry-pose':self.pose_sources.add(spec['id'])
    def publish_sensor(self,identity,t,data,raw):
        value=dict(kind='sensor_sample',sensor_id=identity,timestamp_ns=t,data=data,bytes=base64.b64encode(raw).decode())
        with self.lock:
            if identity in self.pose_sources:self.poses.append(value)
            else:self.samples[identity]=value
            if identity=='lidar.base':self.lidar=(t,json.loads(data))
    def drain(self):
        with self.lock:
            values=self.specs+list(self.poses)+list(self.samples.values());self.specs=[];self.poses.clear();self.samples={}
        for value in values:emit(value)

def emit(value):
    print('SENSOR '+json.dumps(value,separators=(',',':'),allow_nan=False),file=OUTPUT,flush=True)

def main():
    global OUTPUT
    OUTPUT=os.fdopen(os.dup(1),"w",buffering=1)
    # Keep native camera/codec stdout off the framed observation pipe.
    with open(os.devnull,"w") as sink:os.dup2(sink.fileno(),1)
    import faulthandler
    faulthandler.enable()
    import importlib.util
    spec=importlib.util.spec_from_file_location('rgbd_helper',Path(__file__).with_name('capture-rgbd.py'))
    h=importlib.util.module_from_spec(spec);spec.loader.exec_module(h)
    signal.signal(signal.SIGTERM,h.terminate)
    from galbot_sdk.g1 import GalbotRobot,SensorType
    from camera_capture import CameraCaptures
    from sensor_observations import SensorObservations,CAMERAS
    parent=os.getppid();robot=GalbotRobot();cameras=None;sensors=None
    odometry=deque(maxlen=600); odometry_lock=threading.Lock(); stopping=threading.Event()
    def read_odometry():
        last=0
        while not stopping.is_set():
            sample=robot.get_odom()
            if sample and sample["timestamp_ns"]>last:
                last=sample["timestamp_ns"]
                with odometry_lock:odometry.append(sample)
            stopping.wait(.025)
    mapping_camera=os.environ.get('GALBOT_MAPPING_CAMERA','right_arm')
    if mapping_camera not in ('left_arm','right_arm'):raise ValueError('unsupported mapping camera')
    publisher=Publisher();contract=json.loads(os.environ['GALBOT_CAPTURE_CONTEXT'])
    try:
        enabled={SensorType.BASE_LIDAR,SensorType.LEFT_ARM_DEPTH_CAMERA,SensorType.RIGHT_ARM_DEPTH_CAMERA,SensorType.CHASSIS_IMU}
        if not robot.init(enabled,True):raise RuntimeError('read-only sensors initialization failed')
        threading.Thread(target=read_odometry,daemon=True).start()
        cameras=CameraCaptures(CAMERAS,mapping_camera=mapping_camera)
        ctx=SimpleNamespace(galbot=SimpleNamespace(robot=robot),camera_captures=cameras)
        sensors=SensorObservations(ctx,publisher,contract,Path(os.environ['GALBOT_SENSOR_STATE']))
        last_odom=0;last_camera=0;last_lidar=0;last_scan=0
        while os.getppid()==parent:
            started=time.monotonic()
            with odometry_lock:poses=list(odometry);odometry.clear()
            for sample in poses:
                if sample['timestamp_ns']<=last_odom:raise RuntimeError('odometry clock reset')
                last_odom=sample['timestamp_ns'];q=sample['orientation'];norm=sum(v*v for v in q)**.5
                emit(dict(kind='odometry',timestamp_ns=last_odom,pose=dict(translation=sample['position'],rotation_xyzw=[v/norm for v in q])))
            publisher.drain()
            try:
                camera=cameras.mapping(mapping_camera)
                if camera and camera['timestamp_ns']>last_camera:
                    last_camera=camera['timestamp_ns'];emit(dict(kind='camera',sample=camera))
            except Exception as e:emit(dict(kind='status',detail='Camera: '+str(e)))
            with publisher.lock:lidar=publisher.lidar
            if lidar and started-last_lidar>=1 and lidar[0]>last_scan:
                last_lidar=started;last_scan,reading=lidar
                pose,stamp=robot.get_transform('base_link','lidar_base_link',last_scan)
                if len(pose)==7 and stamp==last_scan:
                    emit(dict(kind='lidar',timestamp_ns=last_scan,reading=reading,sensor_to_base=dict(translation=pose[:3],rotation_xyzw=pose[3:])))
            time.sleep(max(.001,.05-(time.monotonic()-started)))
    finally:
        stopping.set()
        if sensors:sensors.close()
        if cameras:cameras.close()
        robot.request_shutdown();robot.destroy()
if __name__=='__main__':main()
