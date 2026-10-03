"""Run with the Galbot Python environment (numpy, OpenCV, pyrealsense2)."""
import importlib.util
from pathlib import Path
import unittest
import numpy as np
spec = importlib.util.spec_from_file_location('capture_rgbd', Path(__file__).resolve().parents[1] / 'scripts/capture-rgbd.py')
rgbd = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rgbd)

class ProjectionTests(unittest.TestCase):
    def setUp(self):
        self.info = dict(width=48,height=32,K=[40.,0.,24.,0.,40.,16.,0.,0.,1.], D=[0.]*5,distortion_model='none')
        self.depth = np.full((32,48),.3)
        self.color = np.zeros((32,48,3), dtype=np.uint8)
        self.color[:] = [9,83,241]

    def test_metres_optical_axes_and_rgb_order(self):
        points,colors = rgbd.project(self.depth,self.color,self.info)
        self.assertEqual(len(points),384)
        np.testing.assert_allclose(points[0],[(2-24)*.3/40,(2-16)*.3/40,.3],atol=1e-7)
        self.assertEqual(colors,[[9,83,241]]*384)

    def test_invalid_far_and_discontinuous_depth_is_not_geometry(self):
        for value in [0.,.6,float('nan')]:
            with self.assertRaises(ValueError):
                rgbd.project(np.full((32,48),value),self.color,self.info)
        self.depth[1:4,1:4]=0
        points,colors = rgbd.project(self.depth,self.color,self.info)
        self.assertLess(len(points),384)
        self.assertEqual(len(points),len(colors))

    def test_unknown_distortion_and_mismatched_dimensions_are_rejected(self):
        self.info['distortion_model']='unknown'
        with self.assertRaises(ValueError):rgbd.project(self.depth,self.color,self.info)
        with self.assertRaises(ValueError):rgbd.project(self.depth,self.color[:20],self.info)

if __name__=='__main__':unittest.main()
