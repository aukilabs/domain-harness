"""Read-only CAD envelope adapter. No control writers or guessed joint poses.

Uses installed BBOS URDF/meshes and calibrated motor-to-URDF conversions.
The vendor health viewer aligns URDF X-forward/Y-left/Z-up to the depth base
X-right/Y-forward/Z-up by a +90 degree Z rotation, at the same ground origin.
Visual mesh convex hulls are approximate body envelopes, not certified collision
geometry. Every snapshot replaces the previous one.
"""
import hashlib
import time
import xml.etree.ElementTree as ET
from pathlib import Path
import numpy as np
from scipy.spatial.transform import Rotation
from robot_occupancy import Solid, origin, transform, stl_vertices, SIZE, MAX_CELLS


class Model:
    def __init__(self, path):
        path = Path(path)
        raw = path.read_bytes()
        xml = ET.fromstring(raw)
        digest = hashlib.sha256(b'bracketbot-cad-envelope/v1;urdf-to-base=Rz(90);voxel=.05;')
        digest.update(raw)
        self.links = {link.get('name'): link for link in xml.findall('link')}
        self.joints = {j.find('child').get('link'): j for j in xml.findall('joint')}
        roots = set(self.links)-set(self.joints)
        if roots != {'root'}:
            raise ValueError('Unexpected BBOS URDF root')
        self.parts = []
        for name, link in self.links.items():
            for visual in link.findall('visual'):
                mesh = visual.find('geometry/mesh')
                if mesh is None:
                    raise ValueError('Unsupported BBOS visual geometry')
                uri = mesh.get('filename')
                if not uri.startswith('package://'):
                    raise ValueError('Expected packaged BBOS mesh')
                asset = (path.parent / uri.removeprefix('package://').split('/', 1)[1]).resolve()
                if not asset.is_relative_to(path.parent.resolve()):
                    raise ValueError('Mesh outside model directory')
                points, content = stl_vertices(asset)
                digest.update(uri.encode()); digest.update(content)
                points *= np.fromstring(mesh.get('scale', '1 1 1'), sep=' ')
                self.parts.append((name, Solid(transform(points, origin(visual.find('origin'))))))
        self.sha256 = digest.hexdigest()

    def poses(self, positions):
        root = np.eye(4)
        root[:3, :3] = Rotation.from_euler('z', 90, degrees=True).as_matrix()
        poses = {'root': root}
        pending = dict(self.joints)
        def angle(joint, seen=()):
            name = joint.get('name')
            if name in seen: raise ValueError('Cyclic mimic joint')
            mimic = joint.find('mimic')
            if mimic is not None:
                source = next(j for j in self.joints.values() if j.get('name') == mimic.get('joint'))
                q = angle(source, (*seen, name))
                return None if q is None else q*float(mimic.get('multiplier', '1'))+float(mimic.get('offset', '0'))
            return positions.get(name)
        while pending:
            progressed = False
            for child, joint in list(pending.items()):
                parent = joint.find('parent').get('link')
                if parent not in poses: continue
                progressed = True
                del pending[child]
                if poses[parent] is None:
                    poses[child] = None; continue
                motion = np.eye(4)
                kind = joint.get('type')
                if kind != 'fixed':
                    q = angle(joint)
                    if q is None:
                        poses[child] = None; continue
                    if not np.isfinite(q): raise ValueError('Nonfinite joint state')
                    axis = np.fromstring(joint.find('axis').get('xyz'), sep=' ')
                    axis /= np.linalg.norm(axis)
                    if kind == 'prismatic': motion[:3, 3] = axis*q
                    elif kind in ('revolute', 'continuous'): motion[:3, :3] = Rotation.from_rotvec(axis*q).as_matrix()
                    else: raise ValueError('Unsupported joint type')
                poses[child] = poses[parent] @ origin(joint.find('origin')) @ motion
            if not progressed: raise ValueError('Disconnected or cyclic URDF')
        return poses

    def sample(self, positions, session):
        started = time.monotonic()
        poses = self.poses(positions)
        chunks, covered, missing = [], set(), set()
        for name, solid in self.parts:
            pose = poses[name]
            if pose is None: missing.add(name); continue
            chunks.append(solid.cells(pose)); covered.add(name)
        cells = np.unique(np.concatenate(chunks), axis=0) if chunks else np.empty((0, 3), dtype=int)
        if len(cells) > MAX_CELLS: raise ValueError('Body volume exceeds cell budget')
        return dict(session_id=session, model_sha256=self.sha256, voxel_size_m=SIZE,
                    grid_origin_m=[0., 0., 0.], cells=cells.tolist(), covered_links=sorted(covered),
                    missing_links=sorted(missing), approximate_links=sorted(covered),
                    compute_ms=(time.monotonic()-started)*1000)


def spec(context, model):
    return dict(id='robot.occupancy', clock=context['clock'], frame_definition=context['base_frame_definition'],
        payload=dict(kind='structured', datatype='spatial.robot-occupancy', schema='spatial.robot-occupancy/v1',
                     modality='robot_occupancy', observes='robot', unit='meters'),
        metadata=dict(model_sha256=model.sha256, session_id=context['session_id'], update_hz=2,
                      geometry='approximate convex envelopes of installed BBOS visual meshes',
                      max_joint_age_ms=250, urdf_to_base='Rz(+90 degrees), shared ground origin'))
