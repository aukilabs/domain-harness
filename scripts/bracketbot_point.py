"""Pure planning for pointing one Bracketbot arm straight ahead or at a point.

Follows bracketbot-runner/docs/arm-control.md: arm_base frame (X forward,
Y left, Z up, origin on the drive axle), motor turns, and moves that pass
through the hanging sides HOME pose.

A straight pointing arm is built from FK alone rather than with hybrid_ik,
which uses the mast's redundancy to bend the elbow (it lands near BBOS's
"camera forward" cfg.home). Starting from HOME, where the arm and wrist hang
straight, one shoulder joint swings the whole arm up to horizontal, the wrist
trims the finger axis, and the mast sets the height. To point at a target,
bounded aim steps then turn that straight arm until the fingers line up.
"""
import numpy as np

from bracketbot_aim import AIM_JOINTS, aim_error, aim_step, hand_axis

# Unpowered rest, arms by the sides (set_arm_pose.py HOME, captured 2026-09-16).
HOME = {
    'arm_left': np.array([1.42, 0., 0., 0., 0., 0., 0., -.088]),
    'arm_right': np.array([-1.42, -.01, 0., 0., 0., 0., 0., .095]),
}
WHEEL_RADIUS = .165/2  # floor z = arm_base z + WHEEL_RADIUS
FORWARD = np.array([1., 0., 0.])
AXIS_TOL_DEG = 3.
SHOULDER_JOINTS = (1, 2)
WRIST_JOINTS = (5, 6)


def axis_error_deg(fk, q, direction=FORWARD):
    return float(np.degrees(np.arccos(np.clip(np.dot(hand_axis(fk(q)[1]), direction), -1, 1))))


def swing_forward(fk, home, lower, upper, joints=SHOULDER_JOINTS):
    """Swing the hanging arm forward to horizontal with one shoulder joint.

    For each candidate joint, find the angle that puts the hand furthest
    ahead; the straight arm is then horizontal. Keep the joint whose fingers
    end up closest to forward (a sideways swing leaves them pointing down).
    Returns (q, joint, finger_error_deg).
    """
    g = (5**.5-1)/2
    best = None
    for joint in joints:
        def ahead(t):
            probe = home.copy(); probe[joint] = t
            return float(fk(probe)[0][0])
        grid = np.linspace(lower[joint], upper[joint], 241)
        i = int(np.argmax([ahead(t) for t in grid]))
        a, b = grid[max(i-1, 0)], grid[min(i+1, len(grid)-1)]
        for _ in range(40):  # golden-section refinement
            c, d = b-g*(b-a), a+g*(b-a)
            a, b = (a, d) if ahead(c) > ahead(d) else (c, b)
        q = home.copy(); q[joint] = (a+b)/2
        err = axis_error_deg(fk, q)
        if best is None or err < best[2]:
            best = (q, joint, err)
    return best


def trim_wrist(fk, q, lower, upper, joints=WRIST_JOINTS):
    """Small wrist corrections so the finger axis is exactly forward."""
    q, j = q.copy(), list(joints)
    for _ in range(80):
        current = hand_axis(fk(q)[1])
        if np.linalg.norm(FORWARD-current) < 1e-5:
            break
        columns = []
        for joint in joints:
            probe = q.copy(); probe[joint] += 1e-4
            columns.append((hand_axis(fk(probe)[1])-current)/1e-4)
        delta = np.linalg.lstsq(np.array(columns).T, FORWARD-current, rcond=None)[0]
        q[j] = np.clip(q[j]+np.clip(delta, -.01, .01), lower[j], upper[j])
    return q


def plan_point(fk, home, lower, upper, side='arm_right', height_m=None):
    """Joint target (motor turns) that points `side`'s straight arm ahead.

    `height_m` is the hand height above the floor; by default the mast stays
    where HOME has it. Joint limits and the swept path from HOME are checked.
    """
    home = np.asarray(home, float)
    lower, upper = np.asarray(lower, float), np.asarray(upper, float)
    if np.max(np.abs(home[1:7])) > .06:
        raise ValueError('pointing plan starts from the straight hanging HOME pose')
    q, joint, swing_err = swing_forward(fk, home, lower, upper)
    if swing_err > 25:
        raise ValueError(f'no shoulder joint swings the arm forward (best {swing_err:.1f} deg)')
    q = trim_wrist(fk, q, lower, upper)
    if height_m is not None:
        probe = q.copy(); probe[0] += .01
        rate = (fk(probe)[0][2]-fk(q)[0][2])/.01  # metres per mast turn
        if abs(rate) < 1e-4:
            raise ValueError('mast does not change hand height')
        q[0] += (height_m-WHEEL_RADIUS-fk(q)[0][2])/rate
    if np.any(q[:7] < lower[:7]) or np.any(q[:7] > upper[:7]):
        raise ValueError('pointing pose is outside joint limits')
    err = axis_error_deg(fk, q)
    if err > AXIS_TOL_DEG:
        raise ValueError(f'fingers miss straight ahead by {err:.1f} deg')
    hand = np.asarray(fk(q)[0], float)
    check_path(fk, home, q, side)
    return q, dict(shoulder_joint=joint, axis_error_deg=err,
                   hand_floor_m=(hand+[0, 0, WHEEL_RADIUS]).tolist(),
                   reach_m=float(hand[0]-np.asarray(fk(home)[0])[0]))


AIM_TOL_DEG = 3.
AIM_STEP_TURNS = .005
AIM_ITERATIONS = 800


def plan_point_at(fk, home, lower, upper, side, target):
    """Joint targets that point `side`'s straight arm at `target` (arm_base, m).

    Returns (forward q, aimed q, report). The arm reaches forward from HOME,
    then aims; both sweeps are checked against joint limits and the clear
    envelope. Refuses a target the fingers cannot line up with.
    """
    target = np.asarray(target, float)
    if target.shape != (3,) or not np.isfinite(target).all():
        raise ValueError('target must be a finite arm_base point')
    forward, report = plan_point(fk, home, lower, upper, side)
    # Shoulder first, then trim with the forearm and wrist: aiming with every
    # joint at once lets the wrist run to its stop before the arm lifts.
    q = forward.copy()
    for joints in (SHOULDER_JOINTS, AIM_JOINTS):
        previous = None
        for _ in range(AIM_ITERATIONS):
            q, err, moved = aim_step(fk, q, target, lower, upper, AIM_STEP_TURNS, side, joints=joints)
            if not moved or err < .2 or previous is not None and previous-err < 1e-4:
                break
            previous = err
    err = aim_error(fk, q, target)[1]
    if err > AIM_TOL_DEG:
        raise ValueError(f'fingers cannot line up with the target (best {err:.1f} deg off)')
    if np.any(q[:7] < lower[:7]) or np.any(q[:7] > upper[:7]):
        raise ValueError('aimed pose is outside joint limits')
    check_path(fk, forward, q, side)
    hand = np.asarray(fk(q)[0], float)
    return forward, q, dict(report, aim_error_deg=err, aimed_hand_floor_m=(hand+[0, 0, WHEEL_RADIUS]).tolist())


def check_path(fk, start, target, side, samples=81):
    """Reject joint-space sweeps that cross the body centre, go behind the
    robot, or drop toward the floor."""
    sign = -1 if side == 'arm_right' else 1
    for t in np.linspace(0, 1, samples):
        p = np.asarray(fk(start+t*(target-start))[0], float)
        if (not np.isfinite(p).all() or sign*p[1] < .05 or p[0] < -.15
                or p[2]+WHEEL_RADIUS < .25):
            raise ValueError(f'path to pointing pose leaves the clear envelope at {np.round(p, 3).tolist()}')


def smootherstep(t):
    t = float(np.clip(t, 0., 1.))
    return t*t*t*(t*(6*t-15)+10)
