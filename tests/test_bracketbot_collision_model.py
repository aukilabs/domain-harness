from pathlib import Path
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
import numpy as np
import trimesh

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_collision_model import build, mesh_path


class CollisionModelTests(unittest.TestCase):
    def test_build_adds_conservative_shapes_without_editing_factory_model(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); installed = root/'installed'; installed.mkdir()
            original_mesh = trimesh.creation.box([.1, .2, .3])
            original_mesh.export(installed/'hand.stl')
            xml = '<robot name="test"><link name="hand"><visual><origin xyz="1 2 3"/><geometry><mesh filename="package://robot/hand.stl" scale="2 1 1"/></geometry></visual></link></robot>'
            source = installed/'robot.urdf'; source.write_text(xml)
            report = build(source, root/'candidate')
            self.assertEqual(source.read_text(), xml)
            self.assertFalse(report['actuation_enabled'])
            self.assertEqual(report['added_collision_shapes'], 1)
            generated = ET.parse(root/'candidate/robot.candidate.urdf').getroot()
            self.assertEqual(generated.find('link/collision/origin').get('xyz'), '1 2 3')
            hull = trimesh.load_mesh(root/'candidate/hull-000.obj')
            np.testing.assert_allclose(hull.extents, [.2, .2, .3], atol=1e-7)
            with self.assertRaises(FileExistsError): build(source, root/'candidate')
            with self.assertRaises(ValueError): build(source, installed/'unsafe')

    def test_mesh_cannot_escape_model_directory(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp)/'robot.urdf'
            for uri in ('https://example.com/mesh.stl', '/etc/passwd', 'package://robot/../outside.stl'):
                with self.assertRaises(ValueError): mesh_path(uri, source)


if __name__ == '__main__':
    unittest.main()
