"""Build an OFFLINE collision-model candidate, never install it into BBOS.

Convex hulls contain each supplied visual mesh, but CAD completeness, physical
clearance, link-pair exclusions and environment sensing still need validation.
This artifact is not a certification and cannot enable actuation.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import xml.etree.ElementTree as ET

import numpy as np
from scipy.spatial import ConvexHull
import trimesh


def mesh_path(uri, source):
    # Installed BBOS package://<package>/meshes/... resolves under URDF dir.
    if uri.startswith('package://'):
        parts = uri.removeprefix('package://').split('/', 1)
        if len(parts) != 2:
            raise ValueError('invalid package mesh')
        path = source.parent/parts[1]
    elif '://' in uri or Path(uri).is_absolute():
        raise ValueError('external mesh location requires explicit resolution')
    else:
        path = source.parent/uri
    path = path.resolve()
    if not path.is_relative_to(source.parent.resolve()) or not path.is_file():
        raise ValueError('missing mesh or mesh outside installed model directory')
    return path


def build(source, output):
    source, output = Path(source).resolve(), Path(output).resolve()
    # Require a new artifact directory: never overwrite the installed model or
    # any earlier reviewed candidate. The caller chooses a scratch location.
    if output == source.parent or output.is_relative_to(source.parent):
        raise ValueError('candidate must be outside installed model directory')
    output.mkdir(parents=False, exist_ok=False)
    root = ET.fromstring(source.read_bytes())
    digest = hashlib.sha256(source.read_bytes())
    rows = []
    original_shapes = sum(len(link.findall('collision')) for link in root.findall('link'))
    for link in root.findall('link'):
        visuals = link.findall('visual')
        # Keep original nonvisual collision bodies, and add visual hulls rather
        # than assuming original collision shapes enclose every physical part.
        for index, visual in enumerate(visuals):
            geometry = visual.find('geometry')
            if geometry is None or len(geometry) != 1:
                raise ValueError('missing or ambiguous visual geometry')
            shape = geometry[0]
            collision = ET.Element('collision', name=f'candidate-{len(rows)}')
            if visual.find('origin') is not None:
                collision.append(copy.deepcopy(visual.find('origin')))
            new_geometry = ET.SubElement(collision, 'geometry')
            row = {'link': link.get('name'), 'visual_index': index}
            if shape.tag == 'mesh':
                path = mesh_path(shape.get('filename', ''), source)
                raw = path.read_bytes(); digest.update(raw)
                scale = np.fromstring(shape.get('scale', '1 1 1'), sep=' ')
                if scale.shape != (3,) or not np.isfinite(scale).all() or np.any(scale <= 0):
                    raise ValueError('invalid mesh scale')
                mesh = trimesh.load_mesh(path, process=False)
                points = np.asarray(mesh.vertices, dtype=float)*scale
                if points.ndim != 2 or points.shape[1] != 3 or not np.isfinite(points).all():
                    raise ValueError('invalid mesh vertices')
                hull = ConvexHull(points)
                # Bound temporary memory on the Jetson for detailed CAD meshes.
                chunk_size = max(1, min(4096, 1_000_000//len(hull.equations)))
                for offset in range(0, len(points), chunk_size):
                    residual = points[offset:offset+chunk_size] @ hull.equations[:, :3].T + hull.equations[:, 3]
                    if float(np.max(residual)) > 1e-7:
                        raise ValueError('candidate hull does not enclose source vertices')
                hull_mesh = trimesh.Trimesh(vertices=points, faces=hull.simplices, process=True).convex_hull
                asset = output/f'hull-{len(rows):03d}.obj'
                hull_mesh.export(asset)
                ET.SubElement(new_geometry, 'mesh', filename=str(asset))
                # Keep the diagnostic URDF's visual asset paths resolvable too.
                shape.set('filename', str(path))
                row.update(source_sha256=hashlib.sha256(raw).hexdigest(),
                           hull_file=asset.name, vertices=int(len(hull_mesh.vertices)),
                           source_vertices=int(len(points)))
            elif shape.tag in ('box', 'cylinder', 'sphere'):
                new_geometry.append(copy.deepcopy(shape))
                row['primitive'] = shape.tag
            else:
                raise ValueError('unsupported visual shape: '+shape.tag)
            link.append(collision)
            rows.append(row)
    if not rows:
        raise ValueError('model has no physical visual geometry')
    report = {'schema': 'spatial.bracketbot-collision-candidate/v1',
              'source_urdf': str(source), 'source_digest': digest.hexdigest(),
              'actuation_enabled': False, 'status': 'unvalidated_candidate',
              'original_collision_shapes': original_shapes,
              'added_collision_shapes': len(rows),
              'unverified': ['physical CAD completeness', 'collision padding',
                             'self-collision pair exclusions', 'continuous swept-path collision',
                             'fresh environment coverage'], 'geometry': rows}
    ET.ElementTree(root).write(output/'robot.candidate.urdf', encoding='utf-8', xml_declaration=True)
    (output/'manifest.json').write_text(json.dumps(report, indent=2)+'\n')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--urdf', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True, help='New scratch directory; must not already exist')
    args = parser.parse_args()
    report = build(args.urdf, args.output)
    print(json.dumps({key: value for key, value in report.items() if key != 'geometry'}))
