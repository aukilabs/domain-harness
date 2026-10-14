"""Aim a straight Bracketbot arm's finger axis at a point. Pure math, no BBOS.

Points are in arm_base, the frame the arm FK/IK uses (bracketbot-runner
docs/arm-control.md): X forward, Y left, Z up, metres, origin on the drive
axle. The caller resolves targets into it through declared transforms.

The aim keeps the elbow (J3) straight and the mast (J0) still, and turns the
shoulder, forearm twist and wrist so the finger axis passes through the target.
"""
import numpy as np

AXLE_HEIGHT_M = .165/2
AIM_JOINTS = (1, 2, 4, 5, 6)
MIN_TARGET_DISTANCE_M = .2


def hand_axis(quaternion):
    """The hand's +Z (along the fingers) in arm_base, from its xyzw quaternion."""
    x, y, z, w = quaternion
    return np.array([2*(x*z+w*y), 2*(y*z-w*x), 1-2*(x*x+y*y)])


def aim_error(fk, q, target):
    """Unit (desired - current) finger direction and its angle in degrees."""
    pos, quat = fk(q)
    to_target = np.asarray(target, float)-np.asarray(pos, float)
    distance = np.linalg.norm(to_target)
    if distance < MIN_TARGET_DISTANCE_M:
        raise ValueError('target is too close to the hand to aim at')
    want, have = to_target/distance, hand_axis(quat)
    return want-have, float(np.degrees(np.arccos(np.clip(np.dot(want, have), -1, 1))))


def in_envelope(fk, q, side='arm_right'):
    """Hand stays on its own side, not behind the robot, and off the floor."""
    p = np.asarray(fk(q)[0], float)
    sign = -1 if side == 'arm_right' else 1
    return bool(np.isfinite(p).all() and sign*p[1] >= .05 and p[0] >= -.15 and p[2]+AXLE_HEIGHT_M >= .25)


def aim_step(fk, q, target, lower, upper, max_step, side='arm_right', damping=.05, joints=AIM_JOINTS):
    """One bounded damped-least-squares step toward pointing at `target`.

    Returns (next q, aim error in degrees before the step, moved). A step that
    would leave the joint limits is clipped; one that would leave the envelope
    is not taken, so the arm holds instead of reaching somewhere unsafe.
    """
    q = np.asarray(q, float)
    error, degrees = aim_error(fk, q, target)
    columns = []
    for j in joints:
        probe = q.copy(); probe[j] += 1e-4
        columns.append((aim_error(fk, probe, target)[0]-error)/-1e-4)
    jac = np.array(columns).T  # d(have - want)/dq, so stepping along -J^T reduces error
    delta = jac.T @ np.linalg.solve(jac @ jac.T + damping**2*np.eye(3), error)
    delta *= min(1., max_step/max(np.max(np.abs(delta)), 1e-12))
    step = q.copy()
    idx = list(joints)
    step[idx] = np.clip(q[idx]+delta, np.asarray(lower, float)[idx], np.asarray(upper, float)[idx])
    if not in_envelope(fk, step, side):
        return q, degrees, False
    return step, degrees, True
