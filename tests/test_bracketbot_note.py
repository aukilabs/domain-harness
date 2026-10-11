import copy
import importlib.util
from pathlib import Path
import sys
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_note import select_note
from test_bracketbot_pointing import fixtures
spec=importlib.util.spec_from_file_location('select_note_command',Path(__file__).resolve().parents[1]/'scripts/select-bracketbot-reach-note.py')
command=importlib.util.module_from_spec(spec);spec.loader.exec_module(command)


class NoteTests(unittest.TestCase):
    def setUp(self):
        self.scene,_,self.hands=fixtures();self.home=dict(peer_id='home-owner',frame='home')
        self.view=dict(authority_peer_id='home-owner',frame='home',notes=[dict(id='note-id',text='this is dan',position=[10.5,1,.7])])
        self.request=command.request_for(self.view,'this is dan')
    def select(self,request):
        return select_note(self.scene,request,self.hands,self.home,scene_age_ms=0,robot_pose_age_ms=0,arm_age_ms=0)
    def test_note_uses_saved_position_and_nearest_hand_without_phone_feed(self):
        result=self.select(self.request)
        self.assertEqual(result['target_position_m'],[-.5,1,.7]);self.assertEqual(result['arm'],'left')
        self.assertEqual(result['target_kind'],'note');self.assertEqual(result['target_id'],'note-id')
        self.assertNotIn('phone_peer_id',result)
    def test_runtime_selection_is_unique_per_command(self):
        newer=command.request_for(self.view,'THIS IS DAN')
        self.assertNotEqual(newer['request_id'],self.request['request_id'])
        self.view['notes'].append(copy.deepcopy(self.view['notes'][0]))
        with self.assertRaises(ValueError):command.request_for(self.view,'this is dan')
        with self.assertRaises(ValueError):command.request_for(self.view,'missing')
    def test_wrong_home_or_frame_or_note_name_cannot_move_robot(self):
        for field,value in [('frame','other'),('authority_peer_id','other'),('requested_name','other')]:
            r=copy.deepcopy(self.request);r[field]=value
            with self.assertRaises(ValueError):self.select(r)
        self.scene['registries']['frame_transform']['validity']='stale'
        with self.assertRaises(ValueError):self.select(self.request)


if __name__=='__main__':unittest.main()
