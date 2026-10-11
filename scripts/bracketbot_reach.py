"""Checked segments for persistent full-arm reaching. No hardware writes here."""
import numpy as np
from scipy.optimize import least_squares


def reach_goal(hand, phone, side, standoff=.20):
    hand, phone = np.asarray(hand, float), np.asarray(phone, float)
    if hand.shape != (3,) or phone.shape != (3,) or not np.isfinite([hand,phone]).all():
        raise ValueError('invalid hand/phone position')
    if side not in ('left','right') or standoff != .20:
        raise ValueError('unsupported arm or standoff')
    delta=phone-hand; distance=np.linalg.norm(delta)
    if distance <= standoff+.03:
        raise ValueError('phone already within stopping distance')
    # At most 30 cm per segment; further segments can extend within the arm's
    # workspace. The base and mast never chase an unreachable target.
    goal=hand+delta/distance*min(.30, distance-standoff)
    sign=1 if side=='left' else -1
    if not (-.35 <= goal[0] <= .65 and .10 <= sign*goal[1] <= .65 and .40 <= goal[2] <= 1.40):
        raise ValueError('target direction outside same-side arm workspace')
    return goal


def plan_reach(cfg, start, phone, side, lower, upper, steps=40):
    """Sample Cartesian IK with measured joint limits and bounded joint changes.

    This is a bounded, same-side reach envelope, not a full collision model.
    Mast/gripper stay unchanged; shoulders, elbow and wrist may move.
    Positions use factory arm_base (X forward, Y left, Z up).
    """
    start=np.asarray(start,float); lower=np.asarray(lower,float); upper=np.asarray(upper,float)
    if any(q.shape!=(8,) or not np.isfinite(q).all() for q in (start,lower,upper)):
        raise ValueError('invalid motor state/limits')
    if np.any(lower>=upper) or np.any(start[:7]<lower[:7]) or np.any(start[:7]>upper[:7]):
        raise ValueError('starting pose outside measured limits')
    urdf=cfg.q2urdf(start.copy()); position,quat=cfg.ik.fk(urdf[:7]); position=np.asarray(position)
    goal=reach_goal(position,phone,side)
    trajectory=[start.copy()]; positions=[position.copy()];partial=False
    for t in np.linspace(0,1,steps+1)[1:]:
        wanted=position+t*(goal-position)
        # Factory hybrid IK can use the vertical stage to reduce residuals.
        # Solve ONLY the six arm motors against its factory FK instead: mast
        # and gripper are constants, not discarded optimizer outputs.
        def residual(joints):
            motors=start.copy();motors[1:7]=joints
            p,quat=cfg.ik.fk(cfg.q2urdf(motors)[:7])
            # A reach controls position, not precise wrist orientation. A small
            # posture regularizer selects a continuous solution for redundant joints.
            return np.r_[(np.asarray(p)-wanted)*10,(joints-trajectory[-1][1:7])*.05]
        lo=np.maximum(np.maximum(lower[1:7],start[1:7]-.20),trajectory[-1][1:7]-.01)
        hi=np.minimum(np.minimum(upper[1:7],start[1:7]+.20),trajectory[-1][1:7]+.01)
        def jacobian(joints):
            base=residual(joints);columns=[]
            for j in range(6):
                probe=joints.copy();probe[j]+=.0001
                columns.append((residual(probe)-base)/.0001)
            return np.array(columns).T
        solved=least_squares(residual,trajectory[-1][1:7],bounds=(lo,hi),max_nfev=80,
                             ftol=1e-7,xtol=1e-7,gtol=1e-7,jac=jacobian)
        motors=start.copy();motors[1:7]=solved.x
        q=cfg.q2urdf(motors.copy())
        if (motors.shape!=(8,) or not np.isfinite(motors).all()
                or np.any(motors[:7]<lower[:7]) or np.any(motors[:7]>upper[:7])
                or np.max(np.abs(motors-start))>.20000001
                or np.max(np.abs(motors-trajectory[-1]))>.01000001):
            raise ValueError('reach exceeds joint/step bounds')
        actual,actual_q=cfg.ik.fk(q[:7]);actual=np.asarray(actual)
        if not np.isfinite(actual).all() or np.linalg.norm(actual-np.asarray(phone))<.195:
            raise ValueError('invalid reach position or standoff')
        if np.linalg.norm(actual-wanted)>.015:
            # Keep only previously validated samples, never this failed point.
            # An unreachable straight-line endpoint need not discard useful reach.
            progress=np.linalg.norm(position-phone)-np.linalg.norm(positions[-1]-phone)
            if progress>=.03:
                partial=True;goal=positions[-1];break
            raise ValueError(f'reach position/standoff check failed at {t:.3f}: position={np.linalg.norm(actual-wanted):.4f}m distance={np.linalg.norm(actual-np.asarray(phone)):.3f}m')
        sign=1 if side=='left' else -1
        if not (-.35<=actual[0]<=.65 and .075<=sign*actual[1]<=.65 and .40<=actual[2]<=1.40):
            raise ValueError('reach path leaves same-side workspace')
        trajectory.append(motors.copy());positions.append(actual.copy())
    # Piecewise timing bounds EACH segment; easing derivative peaks at 1.5.
    times=[0.]
    for a,b,pa,pb in zip(trajectory,trajectory[1:],positions,positions[1:]):
        times.append(times[-1]+max(.10,1.5*np.max(np.abs(b-a))/.04,1.5*np.linalg.norm(pb-pa)/.20))
    if times[-1]>55:raise ValueError('reach path too long for bounded return')
    return dict(trajectory_motor_turns=[q.tolist() for q in trajectory],times=times,
                trajectory_hand_positions=[p.tolist() for p in positions],
                goal_arm_m=goal.tolist(),standoff_m=.20,phone_arm_m=list(phone),arm=side,partial=partial)


def append_plan(previous,next_plan):
    """Keep the complete checked return path when extending/re-aiming a reach."""
    old=np.asarray(previous['trajectory_motor_turns'][-1]);new=np.asarray(next_plan['trajectory_motor_turns'][0])
    if np.max(np.abs(old-new))>.005:raise ValueError('arm moved during replan')
    q=np.asarray(next_plan['trajectory_motor_turns'][1])
    distance=np.linalg.norm(np.asarray(previous['trajectory_hand_positions'][-1])-next_plan['trajectory_hand_positions'][1])
    extra=max(0.,1.5*np.max(np.abs(q-old))/.04-next_plan['times'][1],
              1.5*distance/.20-next_plan['times'][1])
    times=previous['times']+[previous['times'][-1]+t+extra for t in next_plan['times'][1:]]
    if times[-1]>55:raise ValueError('bounded return path is full')
    return dict(next_plan,times=times,
                trajectory_motor_turns=previous['trajectory_motor_turns']+next_plan['trajectory_motor_turns'][1:],
                trajectory_hand_positions=previous['trajectory_hand_positions']+next_plan['trajectory_hand_positions'][1:])


def compact_return_path(cfg,plan):
    """Validate a joint-space shortcut so re-aiming doesn't accumulate history.

    The path runs from the saved pose to the current endpoint, and can be
    reversed on cancellation. If the shortcut leaves the existing workspace,
    retain the original checked path instead.
    """
    start=np.asarray(plan['trajectory_motor_turns'][0]);end=np.asarray(plan['trajectory_motor_turns'][-1])
    steps=max(40,int(np.ceil(np.max(np.abs(end-start))/.005)))
    joints=np.linspace(start,end,steps+1);positions=[];times=[0.]
    sign=1 if plan['arm']=='left' else -1
    for i,q in enumerate(joints):
        p,_=cfg.ik.fk(cfg.q2urdf(q.copy())[:7]);p=np.asarray(p)
        if not np.isfinite(p).all() or (i and not (-.35<=p[0]<=.65 and .075<=sign*p[1]<=.65 and .40<=p[2]<=1.40)):
            return plan
        if i:times.append(times[-1]+max(.10,1.5*np.max(np.abs(q-joints[i-1]))/.04,1.5*np.linalg.norm(p-positions[-1])/.20))
        positions.append(p)
    if times[-1]>=plan['times'][-1]:return plan
    return dict(plan,times=times,trajectory_motor_turns=joints.tolist(),
                trajectory_hand_positions=[p.tolist() for p in positions])


def plan_extension(cfg,previous,start,target,side,lower,upper):
    """Plan off the motor loop, retaining a bounded validated return path."""
    basis=compact_return_path(cfg,previous)
    follow=plan_reach(cfg,start,target,side,lower,upper)
    return append_plan(basis,follow),basis['times'][-1],follow


def needs_replan(plan,target):
    target=np.asarray(target)
    return (np.linalg.norm(target-np.asarray(plan['goal_arm_m']))>.24
            or np.linalg.norm(target-np.asarray(plan['phone_arm_m']))>.04)


def sample_plan(plan, elapsed):
    times=plan['times']; q=plan['trajectory_motor_turns']
    if elapsed<=0:return np.array(q[0])
    if elapsed>=times[-1]:return np.array(q[-1])
    i=int(np.searchsorted(times,elapsed)-1); t=(elapsed-times[i])/(times[i+1]-times[i])
    t=t*t*(3-2*t)
    return np.array(q[i])+(np.array(q[i+1])-q[i])*t
