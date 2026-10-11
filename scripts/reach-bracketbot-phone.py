#!/usr/bin/env python3
"""Persistent reaching; live localization, no base/gripper motion.

Dry-run unless --execute. On target loss/change, reverse the validated path to
the saved rest pose, then release control. Hardware feedback/ownership faults
release immediately; never attempt blind recovery. No HTTP control input.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import fcntl
import json
import os
from pathlib import Path
import signal
import time
import numpy as np
from bracketbot_arm_control import Arms
from bracketbot_pointing import select_target
from bracketbot_note import select_note
from bracketbot_pointing_plan import base_from_arm
from bracketbot_reach import plan_reach,sample_plan,plan_extension,needs_replan
from bracketbot_behavior import supervisor_fresh,alignment_valid
from bracketbot_capture import timestamp

_cache={}

class RobotUnavailable(ValueError):pass


def cached_json(path):
    stat=path.stat();key=(stat.st_ino,stat.st_mtime_ns,stat.st_size)
    cached=_cache.get(path)
    if cached is None or cached[0]!=key:
        cached=(key,json.loads(path.read_text()));_cache[path]=cached
    return cached[1],stat.st_mtime_ns


def read_intent(state, arms, transforms, note_target=None):
    scene,stamp=cached_json(state/'robot-control.json');home,_=cached_json(state/'home-connection.json')
    robot=scene['registries']['local_mapping'].get('robot_pose');now=time.time_ns()
    if (not alignment_valid(scene,home['frame']) or not robot
            or not 0<=(now-stamp)/1e6<=750
            or not 0<=(now-robot['capture_timestamp_ns'])/1e6<=800):
        raise RobotUnavailable('robot tracking or Home alignment unavailable')
    states=arms.healthy();hands={}
    for side,d in states.items():
        cfg=arms.configs[side];p,_=cfg.ik.fk(cfg.q2urdf(d['pos'].astype(float))[:7])
        hands[side]=(transforms[side]@np.r_[p,1])[:3].tolist()
    now=time.time_ns();robot=scene['registries']['local_mapping'].get('robot_pose')
    if note_target:
        request,_=cached_json(note_target)
        return select_note(scene,request,hands,home,scene_age_ms=(now-stamp)/1e6,
            robot_pose_age_ms=(now-robot['capture_timestamp_ns'])/1e6 if robot else None,
            arm_age_ms=max((now-timestamp(d))/1e6 for d in states.values())),states
    packet,_=cached_json(state/'phone-presence.json');now=time.time_ns()
    if not (packet['schema']=='spatial.robot-phone-input/v1' and packet['authenticated'] is True
            and packet['home_peer_id']==home['peer_id'] and packet['home_frame']==home['frame']
            and packet['robot_peer_id']==scene['authority_peer_id']):
        raise ValueError('phone input owner/frame mismatch')
    age=(now-packet['received_ns'])/1e6
    if not 0<=age<=2000:raise ValueError('stale phone input')
    intent=select_target(scene,packet['presence'],hands,scene_age_ms=(now-stamp)/1e6,
        presence_age_ms=packet['request_age_ms']+(now-packet['received_ns'])/1e6,
        robot_pose_age_ms=(now-robot['capture_timestamp_ns'])/1e6 if robot else None,
        arm_age_ms=max((now-timestamp(d))/1e6 for d in states.values()),expected_home_frame=home['frame'],
        max_phone_age_ms=2000)
    return intent,states


def same_target(original,current):
    return (current.get('status')=='target_selected'
            and all(current.get(k)==original.get(k) for k in ('target_kind','target_id','request_id','phone_peer_id','arm','native_frame','home_frame')))


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--state',type=Path,required=True);ap.add_argument('--controller-pid',type=int)
    ap.add_argument('--note-target',type=Path,help='One operator-selected fixed note, not a live phone')
    ap.add_argument('--execute',action='store_true');args=ap.parse_args()
    if args.execute and not args.controller_pid:ap.error('execution requires supervisor')
    stopped=False
    def stop(*_):
        nonlocal stopped
        stopped=True
    signal.signal(signal.SIGTERM,stop);signal.signal(signal.SIGINT,stop)
    report=dict(pid=os.getpid(),execute=args.execute)
    if args.note_target:report['request_id']=json.loads(args.note_target.read_text())['request_id']
    waiting='waiting_for_note' if args.note_target else 'waiting_for_phone'
    report_path=args.state/('note-reach.json' if args.note_target else 'phone-reach.json')
    def status(phase,**details):
        report.update(phase=phase,updated_ns=time.time_ns(),**details)
        if args.execute:
            tmp=report_path.with_suffix('.tmp');tmp.write_text(json.dumps(report));tmp.replace(report_path)
        print(json.dumps(report),flush=True)
    with (args.state/'portal-stance.lock').open('a') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        arms=Arms(args.state,args.controller_pid)
        planner=ThreadPoolExecutor(max_workers=1)
        try:
            transforms={s:base_from_arm(cfg.urdf_path) for s,cfg in arms.configs.items()}
            status(waiting)
            while not stopped:
                if args.execute:
                    heartbeat=json.loads((args.state/'arm-behavior.status.json').read_text())
                    if not supervisor_fresh(heartbeat,args.controller_pid,os.getppid(),time.time_ns()):
                        raise RuntimeError('controller unavailable')
                try:
                    intent,states=read_intent(args.state,arms,transforms,args.note_target)
                    if intent['status']!='target_selected':raise ValueError(intent['reason'])
                    side=intent['arm'];cfg=arms.configs[side]
                    phone=(np.linalg.inv(transforms[side])@np.r_[intent['target_position_m'],1])[:3]
                    plan=plan_reach(cfg,states[side]['pos'].astype(float),phone,side,*arms.limits[side])
                    latest,latest_states=read_intent(args.state,arms,transforms,args.note_target)
                    if not same_target(intent,latest):raise ValueError('target changed while planning')
                    if any(np.max(np.abs(latest_states[s]['pos']-states[s]['pos']))>.005 for s in states):
                        raise ValueError('arms moved while planning')
                except (OSError,KeyError,TypeError,ValueError) as exc:
                    status(waiting,reason=str(exc))
                    if not args.execute:return
                    time.sleep(.5);continue
                status('planned',request_id=intent.get('request_id'),target_kind=intent['target_kind'],target_id=intent['target_id'],target_name=intent.get('target_name'),arm=side,goal_arm_m=plan['goal_arm_m'],
                       standoff_m=.20,duration_seconds=plan['times'][-1])
                if not args.execute:return
                if stopped:break
                saved={s:d['pos'].astype(float).copy() for s,d in states.items()}
                last_live=latest;last_live_at=time.monotonic()
                arms.acquire(states)
                arms.publish(saved,enable=False)
                until=time.monotonic()+2
                while time.monotonic()<until:
                    arms.publish(saved,arm_age=2);time.sleep(.02)
                elapsed=0.;last=time.monotonic();reason='cancelled'
                held_since=None;pending_plan=None;last_plan_attempt=0.;last_measurement=0.
                status('reaching')
                while not stopped:
                    try:
                        live,_=read_intent(args.state,arms,transforms,args.note_target)
                        if not same_target(intent,live):raise ValueError('target temporarily unavailable')
                        last_live=live;last_live_at=time.monotonic()
                    except RobotUnavailable:
                        reason='target_changed_or_stale';break
                    except (OSError,KeyError,TypeError,ValueError):
                        if last_live.get('phone_age_ms',2000)+(time.monotonic()-last_live_at)*1000>2000:
                            reason='target_changed_or_stale';break
                        live=last_live
                    now=time.monotonic();elapsed=min(plan['times'][-1],elapsed+min(.04,now-last));last=now
                    goals={s:q.copy() for s,q in saved.items()};goals[side]=sample_plan(plan,elapsed)
                    next_hand,_=cfg.ik.fk(cfg.q2urdf(goals[side].copy())[:7])
                    live_phone=(np.linalg.inv(transforms[side])@np.r_[live['target_position_m'],1])[:3]
                    if (np.linalg.norm(np.array(next_hand)-live_phone)<.20
                            or np.linalg.norm(np.array(live['hand_position_m'])-live['target_position_m'])<.20):
                        reason='target_changed_or_stale';break
                    arms.publish(goals)
                    if elapsed>=plan['times'][-1]:
                        if held_since is None:held_since=time.monotonic();status('holding')
                        if time.monotonic()-last_measurement>=1:
                            measured=arms.healthy()[side]['pos'].astype(float)
                            hand,_=cfg.ik.fk(cfg.q2urdf(measured.copy())[:7])
                            initial_hand,_=cfg.ik.fk(cfg.q2urdf(saved[side].copy())[:7])
                            status('holding',measured_hand_arm_m=np.asarray(hand).tolist(),
                                   measured_extension_m=float(np.linalg.norm(np.asarray(hand)-initial_hand)),
                                   joint_travel_turns=(measured-saved[side]).tolist())
                            last_measurement=time.monotonic()
                        if pending_plan is not None and pending_plan.done():
                            try:
                                extended,resume_at,follow=pending_plan.result()
                                actual=arms.healthy()[side]['pos'].astype(float)
                                if (np.max(np.abs(actual-np.asarray(follow['trajectory_motor_turns'][0])))>.005
                                        or np.linalg.norm(live_phone-np.asarray(follow['phone_arm_m']))>.10):
                                    raise ValueError('target/arm moved while replanning')
                                plan=extended;elapsed=resume_at;held_since=None
                                status('reaching',goal_arm_m=plan['goal_arm_m'])
                            except ValueError as exc:status('holding',replan_detail=str(exc))
                            pending_plan=None
                        if (held_since is not None and pending_plan is None
                                and time.monotonic()-last_plan_attempt>=1
                                and needs_replan(plan,live_phone)):
                            # Keep motor/feedback updates running while IK works.
                            measured=arms.healthy()[side]['pos'].astype(float)
                            pending_plan=planner.submit(plan_extension,cfg,plan,measured,live_phone.copy(),side,*arms.limits[side])
                            last_plan_attempt=time.monotonic()
                    time.sleep(.02)
                status('restoring',reason=reason)
                last=time.monotonic()
                while elapsed>0:
                    now=time.monotonic();elapsed=max(0,elapsed-min(.04,now-last));last=now
                    goals={s:q.copy() for s,q in saved.items()};goals[side]=sample_plan(plan,elapsed)
                    arms.publish(goals);time.sleep(.02)
                until=time.monotonic()+1
                while time.monotonic()<until:arms.publish(saved);time.sleep(.02)
                measured=arms.healthy()
                error={s:float(np.max(np.abs(measured[s]['pos'][:7]-saved[s][:7]))) for s in saved}
                if max(error.values())>.02:raise RuntimeError('rest pose not reached')
                status('restored',reason=reason,return_error_turns=error)
                return
            status('restored',reason='cancelled')
        except Exception as exc:
            status('fault',reason=str(exc));raise
        finally:
            planner.shutdown(wait=False,cancel_futures=True)
            arms.close()


if __name__=='__main__':main()
