"""Point a Bracketbot arm at a point the way Quest teleop moves it. Pure math, no BBOS.

quest_teleop (bbapps) plans no joint routes: every tick it hands BBOS's
hybrid_ik one hand pose in arm_base and writes the joints it returns. Here
that pose is a pointing pose: the hand where BBOS's home holds it, reached a
little toward the target, its finger axis (+Z) turned onto the target by the
smallest rotation. As in the app, the arm first eases in joint space to BBOS's
cfg.home (a hanging straight arm is a poor IK start), then the hand travels
to the pointing pose on a straight line while its orientation slerps, eased
and speed-capped, one IK solve per tick warm-started from the last. The whole
path is solved and checked before anything moves, and the return retraces it.

Points and poses are in arm_base (X forward, Y left, Z up, metres, origin on
the drive axle); joints are motor turns. `fk(q) -> (pos, xyzw)`,
`solve(pos, xyzw) -> q or None` and `reset(q)` come from the caller.
"""
import numpy as np

from bracketbot_aim import aim_error, hand_axis, in_envelope

REACH_M = .12               # hand moves this far from its home spot toward the target
MAX_SPEED_MPS = .35         # hand speed cap
MAX_TURN_DPS = 120.         # finger-axis turn rate cap
MIN_SECONDS = 1.2
SOLVE_ITERATIONS = 3        # hybrid_ik takes a few bounded steps per call
# hybrid_ik is relaxed: it trails the line by a few cm (measured 2.7 cm on
# 0181). The commanded joints, not the line, are what the limits and envelope
# checks see.
FOLLOW_M, FOLLOW_DEG = .05, 10.     # every sample
ARRIVE_M, ARRIVE_DEG = .01, 3.      # stop re-solving a sample once this close
AIM_TOL_DEG = 3.            # final finger axis vs the target
MAX_JOINT_TURNS_PER_S = 1.  # slow the path down until no joint outruns this
MAX_RETIMES = 2
READY_TURNS_PER_S = .5      # peak joint speed easing to home
MAST_TURNS_PER_S = .35/2.5  # bracketbot-runner arm-control.md
MIN_READY_SECONDS = .8
LIMIT_SLACK_TURNS = .005    # solver round-off at a calibrated stop


def quat_mul(a, b):
    """Hamilton product a*b of xyzw quaternions (b's rotation, then a's)."""
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return np.array([aw*bx+ax*bw+ay*bz-az*by, aw*by-ax*bz+ay*bw+az*bx,
                     aw*bz+ax*by-ay*bx+az*bw, aw*bw-ax*bx-ay*by-az*bz])


def slerp(q0, q1, s):
    q0, q1 = np.asarray(q0, float)/np.linalg.norm(q0), np.asarray(q1, float)/np.linalg.norm(q1)
    d = float(np.dot(q0, q1))
    if d < 0:
        q1, d = -q1, -d
    if d > .9995:
        out = q0+s*(q1-q0)
        return out/np.linalg.norm(out)
    th = np.arccos(np.clip(d, -1, 1))
    return (np.sin((1-s)*th)*q0+np.sin(s*th)*q1)/np.sin(th)


def turn_axis_onto(quat, direction):
    """`quat` turned by the smallest rotation that puts its +Z on `direction`."""
    quat = np.asarray(quat, float)
    f = hand_axis(quat)
    d = np.asarray(direction, float)/np.linalg.norm(direction)
    axis = np.cross(f, d)
    s, c = float(np.linalg.norm(axis)), float(np.clip(np.dot(f, d), -1, 1))
    if s < 1e-9:
        if c > 0:
            return quat/np.linalg.norm(quat)
        axis, s = np.array([0., 0., 1.]), 1.  # exactly opposite: turn about vertical
    half = .5*np.arctan2(s, c)
    out = quat_mul(np.append(np.sin(half)*axis/s, np.cos(half)), quat)
    return out/np.linalg.norm(out)


def angle_deg(q0, q1):
    return float(np.degrees(2*np.arccos(min(abs(float(np.dot(q0, q1))), 1.))))


def pointing_pose(hand_pos, hand_quat, target):
    """The hand reached REACH_M toward `target`, fingers on it."""
    hand_pos, target = np.asarray(hand_pos, float), np.asarray(target, float)
    pos = hand_pos+REACH_M*(target-hand_pos)/np.linalg.norm(target-hand_pos)
    return pos, turn_axis_onto(hand_quat, target-pos)


def ease(t):
    t = float(np.clip(t, 0., 1.))
    return t*t*t*(t*(6*t-15)+10)


def hand_path(start, goal, rate_hz, slower=1.):
    """Eased straight-line samples from `start` to `goal` pose, speed-capped."""
    (p0, q0), (p1, q1) = start, goal
    p0, p1 = np.asarray(p0, float), np.asarray(p1, float)
    # smootherstep peaks at 1.875x its mean speed; cap the peak.
    seconds = slower*max(MIN_SECONDS, 1.875*np.linalg.norm(p1-p0)/MAX_SPEED_MPS,
                         1.875*angle_deg(q0, q1)/MAX_TURN_DPS)
    n = max(int(np.ceil(seconds*rate_hz)), 1)
    return [(p0+ease(i/n)*(p1-p0), slerp(q0, q1, ease(i/n))) for i in range(1, n+1)]


def follow(fk, solve, start, path, lower, upper, side):
    """IK joints for every path sample, checked; and the largest joint step."""
    previous, samples, peak = start, [], 0.
    for pos, quat in path:
        q = None
        for _ in range(SOLVE_ITERATIONS):
            q = solve(pos, quat)
            if q is None:
                break
            got_pos, got_quat = fk(q)
            if np.linalg.norm(got_pos-pos) < ARRIVE_M and angle_deg(got_quat, quat) < ARRIVE_DEG:
                break
        if q is None or not np.isfinite(q).all():
            raise ValueError('the arm cannot reach that pointing pose')
        q = np.asarray(q, float).copy()
        q[7] = start[7]
        got_pos, got_quat = fk(q)
        if np.linalg.norm(got_pos-pos) >= FOLLOW_M or angle_deg(got_quat, quat) >= FOLLOW_DEG:
            raise ValueError(f'the hand cannot follow the path to point there (off by '
                             f'{100*np.linalg.norm(got_pos-pos):.0f} cm, {angle_deg(got_quat, quat):.0f} deg)')
        if np.any(q[:7] < lower[:7]-LIMIT_SLACK_TURNS) or np.any(q[:7] > upper[:7]+LIMIT_SLACK_TURNS):
            raise ValueError('pointing there needs a joint past its calibrated limit')
        q[:7] = np.clip(q[:7], lower[:7], upper[:7])
        if not in_envelope(fk, q, side):
            raise ValueError(f'path to pointing pose leaves the clear envelope at {np.round(got_pos, 3).tolist()}')
        peak = max(peak, float(np.max(np.abs(q[:7]-previous[:7]))))
        samples.append(q)
        previous = q
    return samples, peak


def ease_to(start, goal, rate_hz, fk, lower, upper, side):
    """Eased joint-space samples from `start` to `goal`, checked like the IK path."""
    start, goal = np.asarray(start, float), np.asarray(goal, float)
    if np.any(goal[:7] < lower[:7]) or np.any(goal[:7] > upper[:7]):
        raise ValueError("BBOS's arm home is outside the calibrated limits")
    # smootherstep peaks at 1.875x its mean speed.
    seconds = max(MIN_READY_SECONDS, 1.875*float(np.max(np.abs(goal[1:7]-start[1:7])))/READY_TURNS_PER_S,
                  1.875*abs(goal[0]-start[0])/MAST_TURNS_PER_S)
    n = max(int(np.ceil(seconds*rate_hz)), 1)
    samples = [start+ease(i/n)*(goal-start) for i in range(1, n+1)]
    for q in samples:
        if not in_envelope(fk, q, side):
            raise ValueError('the way to the arm home leaves the clear envelope')
    return samples


def plan_point_at(fk, solve, reset, start, home, target, lower, upper, side, rate_hz):
    """Joint samples (one per tick) that bring the arm from `start` to point at `target`.

    `home` is BBOS's cfg.home (motor turns), where the Quest app starts
    tracking from. The gripper stays as found. A path whose joints would outrun
    MAX_JOINT_TURNS_PER_S is slowed down; one that still would is a jump
    between IK branches and is refused. Raises ValueError to refuse.
    """
    start, target = np.asarray(start, float), np.asarray(target, float)
    lower, upper = np.asarray(lower, float), np.asarray(upper, float)
    if target.shape != (3,) or not np.isfinite(target).all():
        raise ValueError('target must be a finite arm_base point')
    ready = np.asarray(home, float).copy()
    ready[7] = start[7]
    aim_error(fk, ready, target)  # refuses a target too close to the hand
    goal = pointing_pose(*fk(ready), target)
    easing = ease_to(start, ready, rate_hz, fk, lower, upper, side)
    max_step, slower = MAX_JOINT_TURNS_PER_S/rate_hz, 1.
    for _ in range(MAX_RETIMES+1):
        reset(ready)
        samples, peak = follow(fk, solve, ready, hand_path(fk(ready), goal, rate_hz, slower), lower, upper, side)
        if peak <= max_step:
            break
        slower *= 1.1*peak/max_step
    else:
        raise ValueError('the arm would have to jump to follow that path')
    samples = easing+samples
    # The aim is what must land; hybrid_ik may leave the hand a couple of cm
    # off its goal spot (measured up to 2.7 cm on 0181) with the fingers on target.
    err = aim_error(fk, samples[-1], target)[1]
    if err > AIM_TOL_DEG:
        raise ValueError(f'fingers cannot line up with the target (best {err:.1f} deg off)')
    hand = np.asarray(fk(samples[-1])[0], float)
    return samples, dict(aim_error_deg=err, aimed_hand_floor_m=(hand+[0, 0, .165/2]).tolist(),
                         seconds=len(samples)/rate_hz, ready_seconds=len(easing)/rate_hz,
                         mast_travel_turns=float(abs(samples[-1][0]-start[0])))
