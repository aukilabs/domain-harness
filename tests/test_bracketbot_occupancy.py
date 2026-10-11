"""Pure geometry/FK tests: no hardware or control SDK required."""
import sys
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
import numpy as np
from bracketbot_occupancy import Model, Solid, spec


class BodyTests(unittest.TestCase):
    def model(self):
        model = Model.__new__(Model)
        model.sha256 = 'a'*64
        xml = ET.fromstring('''<robot>
          <joint name="lift" type="prismatic"><parent link="root"/><child link="arm"/>
            <origin xyz="1 0 1"/><axis xyz="0 0 1"/></joint>
          <joint name="finger" type="prismatic"><parent link="arm"/><child link="finger"/>
            <axis xyz="0 1 0"/><mimic joint="lift" multiplier="-1" offset="0.1"/></joint>
        </robot>''')
        model.joints = {j.find('child').get('link'): j for j in xml.findall('joint')}
        points = np.array(np.meshgrid(*[[-.04,.04]]*3,indexing='ij')).reshape(3,-1).T
        model.parts = [(name, Solid(points)) for name in ('root','arm','finger')]
        return model

    def test_native_body_axes_and_measured_mimic(self):
        poses = self.model().poses({'lift': .2})
        np.testing.assert_allclose(poses['arm'][:3,3], [0,1,1.2], atol=1e-12)
        np.testing.assert_allclose(poses['finger'][:3,3], [.1,1,1.2], atol=1e-12)
        np.testing.assert_allclose(poses['root'][:3,:3] @ [1,0,0], [0,1,0], atol=1e-12)

    def test_missing_feedback_omits_entire_moving_branch(self):
        result = self.model().sample({}, 'session')
        self.assertEqual(result['covered_links'], ['root'])
        self.assertEqual(result['missing_links'], ['arm','finger'])
        self.assertEqual(result['approximate_links'], ['root'])

    def test_movement_replaces_volume_and_rejects_invalid_state(self):
        model = self.model()
        before = model.sample({'lift':0.}, 'session')
        after = model.sample({'lift':1.}, 'session')
        self.assertNotEqual(before['cells'], after['cells'])
        self.assertEqual(after['missing_links'], [])
        with self.assertRaises(ValueError): model.sample({'lift':float('nan')}, 'session')

    def test_contract_uses_capture_clock_session_and_body_frame(self):
        context = dict(clock={'id':'capture'},base_frame_definition={'frame_id':'robot.base'},session_id='session')
        value = spec(context, self.model())
        self.assertEqual(value['frame_definition'],context['base_frame_definition'])
        self.assertEqual(value['clock'],context['clock'])
        self.assertEqual(value['metadata']['session_id'],'session')
        self.assertEqual(value['payload']['schema'],'spatial.robot-occupancy/v1')


if __name__ == '__main__': unittest.main()
