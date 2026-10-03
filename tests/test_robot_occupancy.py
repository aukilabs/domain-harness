"""Geometry tests run with numpy/scipy; no vendor SDK or robot needed."""
import sys
import unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts'))
import numpy as np
from scipy.spatial.transform import Rotation
from robot_occupancy import Solid, transform

class OccupancyTests(unittest.TestCase):
    def test_solid_interior_and_thin_rotated_parts_are_covered(self):
        for extents in ([.3,.2,.4],[.3,.001,.4]):
            corners=np.array(np.meshgrid(*[[-e/2,e/2] for e in extents],indexing='ij')).reshape(3,-1).T
            solid=Solid(corners)
            pose=np.eye(4);pose[:3,:3]=Rotation.from_euler('xyz',[.2,.6,1.1]).as_matrix();pose[:3,3]=[-.23,.35,1.24]
            cells={tuple(c) for c in solid.cells(pose)}
            rng=np.random.default_rng(7)
            interior=(rng.random((4000,3))-.5)*extents
            occupied=np.floor(transform(np.concatenate([interior,corners]),pose)/.05).astype(int)
            self.assertTrue(all(tuple(c) in cells for c in occupied))
            self.assertIn(tuple(np.floor(pose[:3,3]/.05).astype(int)),cells)

    def test_motion_replaces_old_cells_and_negative_indices_are_correct(self):
        points=np.array(np.meshgrid(*[[-.04,.04]]*3,indexing='ij')).reshape(3,-1).T
        solid=Solid(points);pose=np.eye(4)
        before={tuple(c) for c in solid.cells(pose)}
        self.assertIn((-1,-1,-1),before)
        pose[0,3]=2
        after={tuple(c) for c in solid.cells(pose)}
        self.assertFalse(before & after)
        self.assertIn((39,-1,-1),after)

    def test_cached_jitter_envelope_still_covers_actual_geometry(self):
        points=np.array(np.meshgrid(*[[-.05,.05]]*3,indexing='ij')).reshape(3,-1).T
        solid=Solid(points);pose=np.eye(4);first=solid.cells(pose)
        pose[0,3]=.0015
        reused=solid.cells(pose)
        self.assertIs(first,reused)
        occupied={tuple(c) for c in reused}
        self.assertTrue(all(tuple(c) in occupied for c in np.floor(transform(points,pose)/.05).astype(int)))
        pose[0,3]=.01
        self.assertIsNot(first,solid.cells(pose))

if __name__=='__main__':unittest.main()
