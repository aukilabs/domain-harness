import sys
from pathlib import Path
import unittest
import importlib.util
import contextlib
import io
import json
import signal
import tempfile
from unittest.mock import patch,MagicMock
import numpy as np
from scipy.spatial.transform import Rotation
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_reach import reach_goal,plan_reach,sample_plan,append_plan,plan_extension,needs_replan
spec=importlib.util.spec_from_file_location('phone_reach',Path(__file__).resolve().parents[1]/'scripts/reach-bracketbot-phone.py')
phone_reach=importlib.util.module_from_spec(spec);spec.loader.exec_module(phone_reach)


class IK:
    def reset(self,q):pass
    def fk(self,q):return np.array([.10,.20,.70])+np.array(q[1:4])*.3,Rotation.from_rotvec(q[4:7]).as_quat()
    def solve(self,p,q):return [0,*((np.array(p)-[.10,.20,.70])/.3),*Rotation.from_quat(q).as_rotvec()]
class Config:
    ik=IK()
    q2urdf=staticmethod(lambda q:np.array(q)*2*np.pi)
    urdf2q=staticmethod(lambda q:np.array(q)/(2*np.pi))


class ReachTests(unittest.TestCase):
    def test_holds_until_cancelled_not_gesture_timeout(self):
        with tempfile.TemporaryDirectory() as directory:
            state=Path(directory);(state/'arm-behavior.status.json').write_text('{}')
            cfg=Config();cfg.urdf_path='unused'
            arms=MagicMock();arms.configs={s:cfg for s in ('left','right')}
            arms.limits={s:(np.full(8,-.5),np.full(8,.5)) for s in arms.configs}
            positions={s:np.zeros(8) for s in arms.configs};clock=[0.];handlers={};held=[]
            def states():return {s:dict(pos=q.copy()) for s,q in positions.items()}
            arms.healthy.side_effect=states
            def intent(*args):
                hand,_=cfg.ik.fk(cfg.q2urdf(positions['left']))
                return dict(status='target_selected',target_kind='note',target_id='note',request_id='persistent',
                            arm='left',native_frame='robot',home_frame='home',target_position_m=[.4,.2,.9],
                            hand_position_m=hand.tolist()),states()
            def publish(goals,**kwargs):
                positions.update({s:q.copy() for s,q in goals.items()})
                report=state/'phone-reach.json'
                if report.exists() and json.loads(report.read_text())['phase']=='holding':
                    held.append(clock[0])
                    if clock[0]-held[0]>30:handlers[signal.SIGTERM]()
                if clock[0]>90:raise RuntimeError('test did not terminate')
            arms.publish.side_effect=publish
            def sleep(seconds):clock[0]+=seconds
            with patch.object(sys,'argv',['reach','--state',str(state),'--controller-pid','123','--execute']), \
                 patch.object(phone_reach,'Arms',return_value=arms), \
                 patch.object(phone_reach,'base_from_arm',return_value=np.eye(4)), \
                 patch.object(phone_reach,'read_intent',side_effect=intent), \
                 patch.object(phone_reach,'supervisor_fresh',return_value=True), \
                 patch.object(phone_reach.signal,'signal',side_effect=lambda s,h:handlers.update({s:h})), \
                 patch.object(phone_reach.time,'monotonic',side_effect=lambda:clock[0]), \
                 patch.object(phone_reach.time,'sleep',side_effect=sleep),contextlib.redirect_stdout(io.StringIO()):
                phone_reach.main()
            self.assertGreater(held[-1]-held[0],30)
            report=json.loads((state/'phone-reach.json').read_text())
            self.assertEqual((report['phase'],report['reason']),('restored','cancelled'))
            np.testing.assert_allclose(positions['left'],np.zeros(8))

    def test_target_expiry_motion_arm_switch_and_frame_switch_cancel(self):
        intent=dict(status='target_selected',phone_peer_id='phone',arm='left',native_frame='robot-session',home_frame='home',phone_position_m=[1,0,1])
        self.assertTrue(phone_reach.same_target(intent,dict(intent)))
        for field,value in [('status','blocked'),('phone_peer_id','other'),('arm','right'),
                            ('native_frame','new-session'),('home_frame','other-home')]:
            changed=dict(intent);changed[field]=value
            self.assertFalse(phone_reach.same_target(intent,changed))
        moved=dict(intent,phone_position_m=[1.2,0,1])
        self.assertTrue(phone_reach.same_target(intent,moved))
    def test_standoff_and_far_phone_extension_cap(self):
        hand=np.array([.1,.2,.7]);phone=np.array([.4,.2,.9])
        goal=reach_goal(hand,phone,'left')
        self.assertAlmostEqual(np.linalg.norm(phone-goal),.20)
        goal=reach_goal(hand,[2,.2,1],'left')
        self.assertAlmostEqual(np.linalg.norm(goal-hand),.30)

    def test_cross_body_too_close_and_nonfinite_rejected(self):
        for p in ([.1,-1,.7],[.11,.2,.7],[float('nan'),.2,.7]):
            with self.assertRaises(ValueError):reach_goal([.1,.2,.7],p,'left')
        with self.assertRaises(ValueError):reach_goal([.1,.2,.5],[.1,.2,-1.],'left')
        self.assertLess(reach_goal([.1,.2,.7],[-1,.4,.8],'left')[0],.1)

    def test_full_arm_trajectory_preserves_mast_and_gripper_and_returns(self):
        start=np.zeros(8);start[0]=.01;start[7]=.03
        plan=plan_reach(Config(),start,[.4,.2,.9],'left',np.full(8,-.5),np.full(8,.5))
        q=np.array(plan['trajectory_motor_turns']);times=plan['times']
        self.assertTrue(np.max(abs(q[-1,1:4]-start[1:4]))>.01)
        np.testing.assert_allclose(q[:,0],start[0]);np.testing.assert_allclose(q[:,7],start[7])
        np.testing.assert_allclose(sample_plan(plan,times[-1]),q[-1])
        np.testing.assert_allclose(sample_plan(plan,0),start)
        self.assertLessEqual(times[-1],55)
        for a,b,dt in zip(q,q[1:],np.diff(times)):
            self.assertLessEqual(1.5*np.max(abs(b-a))/dt,.04000001)

    def test_continuous_reaim_keeps_original_return_without_growing_history(self):
        cfg=Config();limits=(np.full(8,-.5),np.full(8,.5));start=np.zeros(8)
        plan=plan_reach(cfg,start,[.65,.2,.7],'left',*limits)
        # More total adjustments than fit in a 55-second concatenated history.
        for i in range(24):
            target=[.5,.6,.7] if i%2 else [.65,.1,.7]
            previous_end=np.asarray(plan['trajectory_motor_turns'][-1])
            plan,resume,follow=plan_extension(cfg,plan,previous_end,target,'left',*limits)
            np.testing.assert_allclose(sample_plan(plan,resume),previous_end)
            np.testing.assert_allclose(sample_plan(plan,0),start)
            self.assertLessEqual(plan['times'][-1],55)
            for a,b,dt in zip(np.asarray(plan['trajectory_motor_turns']),np.asarray(plan['trajectory_motor_turns'])[1:],np.diff(plan['times'])):
                self.assertLessEqual(1.5*np.max(abs(b-a))/dt,.04000001)

    def test_reaim_and_extension_triggers(self):
        plan=dict(goal_arm_m=[.4,.2,.7],phone_arm_m=[.6,.2,.7])
        self.assertFalse(needs_replan(plan,[.6,.2,.7]))
        self.assertTrue(needs_replan(plan,[.6,.25,.7]))
        self.assertTrue(needs_replan(plan,[1.,.2,.7]))

    def test_unreachable_endpoint_keeps_validated_progress(self):
        cfg=Config();cfg.ik=IK()
        cfg.ik.fk=lambda q:(np.array([min(.22,.10+q[1]*.3),.20+q[2]*.3,.70+q[3]*.3]),np.array([0,0,0,1]))
        plan=plan_reach(cfg,np.zeros(8),[1.,.2,.7],'left',np.full(8,-.5),np.full(8,.5))
        self.assertTrue(plan['partial'])
        self.assertGreater(plan['goal_arm_m'][0],.13)
        self.assertLessEqual(plan['goal_arm_m'][0],.22)
        self.assertLess(len(plan['times']),41)
        np.testing.assert_allclose(plan['goal_arm_m'],plan['trajectory_hand_positions'][-1])

    def test_replan_rejects_discontinuous_join(self):
        plan=plan_reach(Config(),np.zeros(8),[.4,.2,.9],'left',np.full(8,-.5),np.full(8,.5))
        with self.assertRaises(ValueError):append_plan(plan,plan)

    def test_bad_ik_and_out_of_limits_start_rejected(self):
        start=np.zeros(8);start[2]=.6
        with self.assertRaises(ValueError):plan_reach(Config(),start,[.4,.2,.9],'left',np.full(8,-.5),np.full(8,.5))
        cfg=Config();cfg.ik=IK();cfg.ik.fk=lambda q:([float('nan')]*3,[0,0,0,1])
        with self.assertRaises(ValueError):plan_reach(cfg,np.zeros(8),[.4,.2,.9],'left',np.full(8,-.5),np.full(8,.5))


if __name__=='__main__':unittest.main()
