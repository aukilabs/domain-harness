import copy
import sys
import unittest
from pathlib import Path
import numpy as np
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_stance import TARGET, calibrated, ease, hand_axis, plan_wrist


def fk(q):
    # Analytic two-axis wrist, unlike the numerical planner under test.
    yaw, pitch = q[5]*2*np.pi, q[6]*2*np.pi
    rz = np.array([[np.cos(yaw),-np.sin(yaw),0],[np.sin(yaw),np.cos(yaw),0],[0,0,1]])
    ry = np.array([[np.cos(pitch),0,np.sin(pitch)],[0,1,0],[-np.sin(pitch),0,np.cos(pitch)]])
    from bracketbot_capture import rotation_quaternion
    return np.array([.05,.12,.65]), rotation_quaternion(rz@ry@np.diag([1.,-1.,-1.]))


class StanceTests(unittest.TestCase):
    def test_wrist_only_plan_preserves_previous_pose_and_other_joints(self):
        q = np.array([0.,.01,-.01,.02,0,.01,.02,-.1])
        saved = q.copy()
        target = plan_wrist(q, fk, np.full(8,-.3), np.full(8,.3))
        np.testing.assert_array_equal(q, saved)
        np.testing.assert_array_equal(target[[0,1,2,3,4,7]], saved[[0,1,2,3,4,7]])
        np.testing.assert_allclose(hand_axis(fk(target)[1]), TARGET, atol=1e-5)

    def test_unreachable_unsafe_and_nonrest_start_are_rejected(self):
        with self.assertRaises(ValueError):
            plan_wrist(np.zeros(8), fk, np.full(8,-.02), np.full(8,.02))
        q = np.zeros(8); q[3] = .25
        with self.assertRaises(ValueError): plan_wrist(q, fk, np.full(8,-.3), np.full(8,.3))
        with self.assertRaises(ValueError):
            plan_wrist(np.zeros(8), lambda q: (np.array([0,0,.2]), fk(q)[1]), np.full(8,-.3), np.full(8,.3))

    def test_confirmation_requires_new_metric_estimates_and_shared_alignment(self):
        t = 5_000_000_000; clock = {'id':'session-clock'}
        s = {'frame':'session', 'registries':{'local_mapping':{
            'observed_portals':{'HNY3JGNLRAE':{'clock':clock, 'observations':5,
                'capture_timestamp_ns':t, 'reprojection_rms_px':.8, 'latest_measured_pose':{'translation':[0,0,0]}}},
            'robot_pose':{'clock':clock, 'capture_timestamp_ns':t},
            'alignment_to_home':{'status':'aligned','anchor':'HNY3JGNLRAE'}}}}
        check = lambda s, now=t: calibrated(s, 'HNY3JGNLRAE',t-1, now, 'session',clock,2)
        self.assertTrue(check(s))
        self.assertFalse(check(s,t+3_000_000_000))
        for key, value in [('robot_pose',None), ('alignment_to_home',None), ('observed_portals',{})]:
            modified=copy.deepcopy(s); modified['registries']['local_mapping'][key]=value
            self.assertFalse(check(modified))
        for key, value in [('clock',{'id':'old'}),('observations',4),('capture_timestamp_ns',t-2),('reprojection_rms_px',float('nan'))]:
            modified=copy.deepcopy(s); modified['registries']['local_mapping']['observed_portals']['HNY3JGNLRAE'][key]=value
            self.assertFalse(check(modified))
        s['frame']='old-session'; self.assertFalse(check(s))

    def test_ramp_is_bounded_monotonic_and_has_soft_endpoints(self):
        self.assertEqual(ease(-1),0); self.assertEqual(ease(2),1)
        self.assertTrue(all(x<=y for x,y in zip([ease(t) for t in np.linspace(0,1,100)], [ease(t) for t in np.linspace(0,1,100)][1:])))
        self.assertLess(ease(.001)/.001, .01)


if __name__ == '__main__': unittest.main()
