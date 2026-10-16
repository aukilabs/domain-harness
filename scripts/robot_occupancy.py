"""Shared bounded geometry primitives for the Bracketbot body envelope."""
import sys
from pathlib import Path

# Optional private numerical runtime; never replace vendor/system packages.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / 'runtime' / 'occupancy'))
import numpy as np
from scipy.spatial import ConvexHull
from scipy.spatial.transform import Rotation

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
