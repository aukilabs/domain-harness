"""Read-only, 2 Hz collision-envelope voxelization inside the existing runner.

No point-cloud accumulation: every sample replaces the previous body volume.
The grid is fixed in robot.base (ROS body axes), metres, 5 cm cells.
"""
import hashlib
import json
import sys
import time
import xml.etree.ElementTree as ET
from pathlib import Path

# Optional private numerical runtime; never replace vendor/system packages.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / 'runtime' / 'occupancy'))
import numpy as np
from scipy.spatial import ConvexHull
from scipy.spatial.transform import Rotation

MODEL = '/data/config/galbot_description/galbot_g_series_description/galbot_g1_cali_user.urdf'
SIZE = .05
MAX_CELLS = 65536


def origin(element):
    out = np.eye(4)
    if element is not None:
        out[:3, 3] = np.fromstring(element.get('xyz', '0 0 0'), sep=' ')
        out[:3, :3] = Rotation.from_euler('xyz', np.fromstring(element.get('rpy', '0 0 0'), sep=' ')).as_matrix()
    return out


def transform(points, matrix):
    return points @ matrix[:3, :3].T + matrix[:3, 3]


def stl_vertices(path):
    raw = path.read_bytes()
    count = int.from_bytes(raw[80:84], 'little')
    if len(raw) != 84 + count * 50:
        raise ValueError('expected binary collision STL: ' + str(path))
    dtype = np.dtype([('normal', '<f4', (3,)), ('vertices', '<f4', (3, 3)), ('attribute', '<u2')])
    return np.unique(np.frombuffer(raw, dtype=dtype, offset=84)['vertices'].reshape(-1, 3), axis=0).astype(float), raw


class Solid:
    def __init__(self, points, padding=0.):
        hull = ConvexHull(points)
        self.points = points[hull.vertices]
        self.planes = np.unique(np.round(hull.equations, 10), axis=0)
        # Cached link rasterization remains conservative under <=2 mm of
        # bounded vertex motion. Joint jitter must not monopolize acquisition.
        self.padding = padding + .002
        self.radius = np.linalg.norm(self.points, axis=1).max()
        self.cached = None

    def cells(self, pose, size=SIZE):
        if self.cached is not None:
            previous, previous_size, cells = self.cached
            motion = np.linalg.norm(pose[:3, 3] - previous[:3, 3]) + self.radius * np.linalg.norm(pose[:3, :3] - previous[:3, :3])
            if size == previous_size and motion <= .002:
                return cells
        vertices = transform(self.points, pose)
        lo = np.floor((vertices.min(axis=0) - self.padding) / size).astype(int)
        hi = np.floor((vertices.max(axis=0) + self.padding) / size).astype(int)
        if np.prod(hi - lo + 1) > MAX_CELLS:
            raise ValueError('collision geometry exceeds bounded grid')
        cells = np.stack(np.meshgrid(*[np.arange(a, b + 1) for a, b in zip(lo, hi)], indexing='ij'), axis=-1).reshape(-1, 3)
        centers = (cells + .5) * size
        # Reject only when a hull plane separates the entire cube. Conservative
        # at edges/corners; includes interiors and thin parts, never just vertices.
        normals = self.planes[:, :3] @ pose[:3, :3].T
        offsets = self.planes[:, 3] - normals @ pose[:3, 3]
        support = size / 2 * np.abs(normals).sum(axis=1) + self.padding
        for n, d, r in zip(normals, offsets, support):
            keep = centers @ n + d <= r + 1e-8
            cells, centers = cells[keep], centers[keep]
        self.cached = (pose.copy(), size, cells)
        return cells


class Model:
    def __init__(self, path=MODEL):
        path = Path(path)
        raw = path.read_bytes()
        root = ET.fromstring(raw)
        digest = hashlib.sha256(raw)
        self.parts = []
        self.links = []
        joints = {j.find('child').get('link'): j for j in root.findall('joint')}
        grippers = {'left': [], 'right': []}
        for link in root.findall('link'):
            name = link.get('name')
            for collision in link.findall('collision'):
                mesh = collision.find('geometry/mesh')
                if mesh is None:
                    raise ValueError('unsupported collision primitive')
                uri = mesh.get('filename')
                if not uri.startswith('package://'):
                    raise ValueError('expected packaged collision asset')
                mesh_path = path.parent.parent / uri[len('package://'):]
                points, content = stl_vertices(mesh_path)
                digest.update(uri.encode()); digest.update(content)
                points *= np.fromstring(mesh.get('scale', '1 1 1'), sep=' ')
                points = transform(points, origin(collision.find('origin')))
                self.links.append(name)
                if '_gripper_' in name:
                    side = name.split('_')[0]
                    grippers[side].append((name, points))
                else:
                    self.parts.append((name, Solid(points), [name], False))
        # Vendor gripper states lack named joint angles. Publish an explicitly
        # approximate envelope over permitted finger travel, not an invented pose.
        self.approximate = []
        for side, meshes in grippers.items():
            if not meshes:
                continue
            driver = next(j for j in joints.values() if j.get('name') == side + '_gripper_r_knuckle_joint')
            limit = driver.find('limit')
            angles = np.linspace(float(limit.get('lower')), float(limit.get('upper')), 129)
            mount = side + '_arm_end_effector_mount_link'
            cloud = []
            for angle in angles:
                for name, points in meshes:
                    chain = []; child = name
                    while child != mount:
                        joint = joints[child]; chain.append(joint); child = joint.find('parent').get('link')
                    matrix = np.eye(4)
                    for joint in reversed(chain):
                        matrix = matrix @ origin(joint.find('origin'))
                        if joint.get('type') != 'fixed':
                            mimic = joint.find('mimic')
                            q = angle if mimic is None else angle * float(mimic.get('multiplier', '1')) + float(mimic.get('offset', '0'))
                            axis = np.fromstring(joint.find('axis').get('xyz'), sep=' ')
                            rotation = np.eye(4); rotation[:3, :3] = Rotation.from_rotvec(axis * q).as_matrix()
                            matrix = matrix @ rotation
                    cloud.append(transform(points, matrix))
            names = [name for name, _ in meshes]
            self.approximate.extend(names)
            # 1 mm margin bounds interpolation between sweep samples for this
            # sub-0.5 m mechanism (step < .014 rad, at most two moving joints).
            self.parts.append((mount, Solid(np.concatenate(cloud), padding=.001), names, True))
        self.sha256 = digest.hexdigest()


class RobotOccupancy:
    def __init__(self, sensors):
        self.sensors = sensors
        self.model = None

    def poll(self):
        if self.model is None:
            self.model = Model()
        s = self.sensors
        odometry = s.robot.get_odom()
        if not odometry:
            return 0
        t = int(odometry['timestamp_ns']) - 200000000
        identity = 'robot.occupancy'
        if t <= s.last.get(identity, 0):
            return 0
        frame = s.registry.frame_ros_body(s.owner, 'robot.base'); frame.pop('hash', None)
        if identity not in s.specs:
            spec = dict(id=identity, clock=s.contract['clock'], frame_definition=frame,
                payload=dict(kind='structured', datatype='spatial.robot-occupancy', schema='spatial.robot-occupancy/v1',
                    modality='robot_occupancy', observes='robot', unit='meters'),
                metadata=dict(model_sha256=self.model.sha256, session_id=s.contract['session_id'], model_uri=MODEL, update_hz=2,
                    geometry='conservative collision envelope; grippers swept over joint limits',
                    pose_products=['robot.link.' + part[0] + '.pose-history' for part in self.model.parts]))
            s.publisher.register_sensor(json.dumps(spec)); s.specs[identity] = spec
        started = time.monotonic(); chunks = []; covered = []; missing = []
        for name, solid, names, approximate in self.model.parts:
            p, stamp = s.robot.get_transform('base_link', name, t, 0)
            if len(p) != 7 or stamp != t or not np.isfinite(p).all() or abs(np.dot(p[3:], p[3:]) - 1) > 1e-6:
                missing.extend(names); continue
            matrix = np.eye(4); matrix[:3, :3] = Rotation.from_quat(p[3:]).as_matrix(); matrix[:3, 3] = p[:3]
            chunks.append(solid.cells(matrix)); covered.extend(names)
            link_frame = s.registry.frame_ros_body(s.owner, name); link_frame.pop('hash', None)
            s.publish_pose('robot.link.' + name + '.pose', t, link_frame)
        cells = np.unique(np.concatenate(chunks), axis=0) if chunks else np.empty((0, 3), dtype=int)
        if len(cells) > MAX_CELLS:
            raise ValueError('robot occupancy exceeds cell limit')
        sample = dict(session_id=s.contract['session_id'], model_sha256=self.model.sha256,
            voxel_size_m=SIZE, grid_origin_m=[0., 0., 0.], cells=cells.tolist(),
            covered_links=covered, missing_links=missing, approximate_links=self.model.approximate,
            compute_ms=(time.monotonic()-started)*1000)
        s.publisher.publish_sensor(identity, t, json.dumps(sample, allow_nan=False), b'')
        s.last[identity] = t
        with s.lock:
            s.counts[identity] = s.counts.get(identity, 0) + 1
            s.states['robot.occupancy.details'] = dict(cells=len(cells), compute_ms=sample['compute_ms'],
                measurement_age_ms=(time.time_ns()-t)/1e6, missing_links=missing)
        return 1
