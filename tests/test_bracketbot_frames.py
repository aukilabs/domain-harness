import sys
import unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_pointing import point_in_frame

class Frames(unittest.TestCase):
    def test_explicit_robot_home_conversion(self):
        t=dict(from_frame_id='robot',to_frame_id='home',translation=[10,2,3],rotation_wxyz=[1,0,0,0])
        self.assertEqual(point_in_frame([1,0,0],'robot','home',t),(11,2,3))
        self.assertEqual(point_in_frame([11,2,3],'home','robot',t),(1,0,0))
        self.assertEqual(point_in_frame([1,0,0],'robot','robot',None),(1,0,0))
        with self.assertRaises(ValueError):point_in_frame([1,0,0],'unknown','home',t)
        with self.assertRaises(ValueError):point_in_frame([1,0,0],'','home',t)

if __name__=='__main__':unittest.main()
