"""Read-only IK candidate generation, NOT a collision-checked motion controller."""
import xml.etree.ElementTree as ET
import numpy as np
from scipy.spatial.transform import Rotation, Slerp


def base_from_arm(urdf_path):
    """Resolve arm_base through the installed URDF, then its documented Rz90 base mapping."""
    xml = ET.parse(urdf_path).getroot()
    joints = {j.find('child').get('link'): j for j in xml.findall('joint')}
    pose = np.eye(4)
    child, visited = 'arm_base', set()
    while child != 'root':
        if child in visited or child not in joints:
            raise ValueError('invalid arm-base ancestry')
        visited.add(child)
        joint = joints[child]
        if joint.get('type') != 'fixed':
            raise ValueError('arm-base ancestry needs live joint transforms')
        origin = joint.find('origin')
        step = np.eye(4)
        if origin is not None:
            xyz = np.fromstring(origin.get('xyz', '0 0 0'), sep=' ')
            rpy = np.fromstring(origin.get('rpy', '0 0 0'), sep=' ')
            if xyz.shape != (3,) or rpy.shape != (3,) or not np.isfinite([xyz, rpy]).all():
                raise ValueError('invalid URDF origin')
            step[:3, :3] = Rotation.from_euler('xyz', rpy).as_matrix()
            step[:3, 3] = xyz
        pose = step @ pose
        child = joint.find('parent').get('link')
    convention = np.eye(4)
    convention[:3, :3] = Rotation.from_euler('z', 90, degrees=True).as_matrix()
    return convention @ pose


def pointing_rotation(current, direction):
    direction = np.asarray(direction, float)
    length = np.linalg.norm(direction)
    if not np.isfinite(direction).all() or length < .01:
        raise ValueError('invalid pointing ray')
    direction /= length
    old = current.apply([0., 0., 1.])
    cross = np.cross(old, direction)
    sine = np.linalg.norm(cross)
    cosine = np.clip(np.dot(old, direction), -1., 1.)
    if cosine < -.999:
        raise ValueError('near-180 degree pointing swing requires another posture')
    if sine < 1e-9:
        return current
    return Rotation.from_rotvec(cross/sine*np.arctan2(sine, cosine)) * current


def candidate(cfg, start, target_arm, lower, upper, steps=40):
    """Keep hand position, mast and gripper fixed; solve orientation gradually.

    Measured joint limits and sampled FK residuals are checked. This does NOT
    check collisions between samples, with the other arm, or with the room.
    There is deliberately no execute option and no BBOS Writer in this module.
    """
    start = np.asarray(start, float)
    lower, upper = np.asarray(lower, float), np.asarray(upper, float)
    if (start.shape != (8,) or lower.shape != (8,) or upper.shape != (8,)
            or not np.isfinite(start).all() or not np.isfinite(lower).all()
            or not np.isfinite(upper).all() or np.any(lower >= upper)):
        raise ValueError('invalid joints or limits')
    if np.any(start[:7] < lower[:7]) or np.any(start[:7] > upper[:7]):
        raise ValueError('current pose outside measured joint limits')
    if not isinstance(steps, int) or not 10 <= steps <= 100:
        raise ValueError('invalid preview resolution')
    urdf_start = cfg.q2urdf(start.copy())
    position, quat = cfg.ik.fk(urdf_start[:7])
    position = np.asarray(position, float)
    if not np.isfinite(position).all() or not np.isfinite(quat).all():
        raise ValueError('invalid forward kinematics')
    rotation = Rotation.from_quat(quat)
    goal = pointing_rotation(rotation, np.asarray(target_arm)-position)
    interpolation = Slerp([0., 1.], Rotation.from_quat([rotation.as_quat(), goal.as_quat()]))
    cfg.ik.reset(urdf_start[:7].tolist())
    trajectory = [start.copy()]
    errors = []
    for fraction in np.linspace(0, 1, steps+1)[1:]:
        wanted = interpolation(float(fraction))
        solution = np.asarray(cfg.ik.solve(position.tolist(), wanted.as_quat().tolist()), float)
        if solution.shape != (7,) or not np.isfinite(solution).all():
            raise ValueError('IK failed')
        q = urdf_start.copy()
        q[1:7] = solution[1:7]  # Mast and gripper remain at their measured positions.
        motors = np.asarray(cfg.urdf2q(q), float)
        if (motors.shape != (8,) or not np.isfinite(motors).all()
                or np.any(motors[:7] < lower[:7]) or np.any(motors[:7] > upper[:7])
                or np.max(np.abs(motors-start)) > .20
                or np.max(np.abs(motors-trajectory[-1])) > .01):
            raise ValueError('IK trajectory exceeds joint or step bounds')
        actual_position, actual_quat = cfg.ik.fk(q[:7])
        if not np.isfinite(actual_position).all() or not np.isfinite(actual_quat).all():
            raise ValueError('invalid IK residual')
        error = np.degrees((wanted.inv()*Rotation.from_quat(actual_quat)).magnitude())
        if np.linalg.norm(np.asarray(actual_position)-position) > .025 or error > 5:
            raise ValueError('IK did not achieve bounded pointing pose')
        trajectory.append(motors.copy())
        errors.append(float(error))
    return {'status': 'candidate_only', 'actuation_enabled': False,
            'collision_status': 'not_checked', 'minimum_duration_seconds': 8,
            'max_orientation_error_deg': max(errors),
            'trajectory_motor_turns': [q.tolist() for q in trajectory]}
