"""Pure planning and confirmation checks for the bounded Portal search stance."""
import numpy as np

TARGET = np.array([2**-.5, 0., -2**-.5])


def hand_axis(quaternion):
    # Factory arm_base: X forward, Y left, Z up. Hand/EFF +Z follows fingers.
    x, y, z, w = quaternion
    return np.array([2*(x*z+w*y), 2*(y*z-w*x), 1-2*(x*x+y*y)])


def plan_wrist(start, fk, lower, upper):
    """Aim fingers forward/down using only wrist joints 5 and 6 (motor turns).

    This intentionally supports the verified hanging-arm starting envelope only;
    arbitrary poses require a whole-arm collision planner before execution.
    """
    start = np.asarray(start, float)
    lower, upper = np.asarray(lower, float), np.asarray(upper, float)
    if start.shape != (8,) or not np.isfinite(start).all():
        raise ValueError('invalid starting pose')
    if np.max(np.abs(start[1:5])) > .06:
        raise ValueError('search stance requires arms near their hanging rest pose')
    if np.any(start[:7] < lower[:7]-.002) or np.any(start[:7] > upper[:7]+.002):
        raise ValueError('starting arm is outside measured joint limits')
    target = start.copy()
    for _ in range(80):
        _, rotation = fk(target)
        current = hand_axis(rotation)
        if np.linalg.norm(TARGET-current) < 1e-5:
            break
        columns = []
        for joint in (5, 6):
            probe = target.copy(); probe[joint] += .0001
            columns.append((hand_axis(fk(probe)[1])-current)/.0001)
        delta = np.linalg.lstsq(np.array(columns).T, TARGET-current, rcond=None)[0]
        target[5:7] += np.clip(delta, -.01, .01)
        target[5:7] = np.clip(target[5:7], lower[5:7]+.01, upper[5:7]-.01)
    if np.linalg.norm(hand_axis(fk(target)[1])-TARGET) > .01:
        raise ValueError('45 degree wrist target is unreachable within measured limits')
    if np.max(np.abs(target-start)) > .20:
        raise ValueError('wrist movement exceeds 72 degree bound')
    initial, _ = fk(start)
    for t in np.linspace(0, 1, 81):
        point, _ = fk(start+t*(target-start))
        point = np.asarray(point)
        if (not np.isfinite(point).all() or point[2] < .55
                or abs(point[1]) < .075 or abs(point[1]) > .25
                or point[0] < -.08 or point[0] > .25
                or np.linalg.norm(point-initial) > .20):
            raise ValueError('wrist path leaves the verified clear hanging-arm envelope')
    return target


def calibrated(scene, portal, started_ns, now_ns, frame, clock, home_frame, baseline=0):
    """Require multiple new SDK metric estimates, current tracking and Home alignment.

    QR detection alone is insufficient. Reject old evidence, session changes,
    frozen files, and alignment to a different Portal.
    """
    try:
        mapping = scene['registries']['local_mapping']
        e = mapping['observed_portals'][portal]
        pose = mapping['robot_pose']
        alignment = scene['registries']['frame_transform']
        product = scene['registries']['frame_transform_product']
        evidence = alignment['evidence']
        return bool(scene['frame'] == frame and e['clock'] == clock
                    and e['observations'] >= baseline+3
                    and started_ns <= e['capture_timestamp_ns'] <= now_ns+100_000_000
                    and now_ns-e['capture_timestamp_ns'] <= 2_000_000_000
                    and pose['clock'] == clock
                    and -100_000_000 <= now_ns-pose['capture_timestamp_ns'] <= 800_000_000
                    and np.isfinite(e['reprojection_rms_px'])
                    and e['reprojection_rms_px'] <= 3.
                    and e.get('latest_measured_pose')
                    and alignment['schema'] == 'spatial.frame-transform/v1'
                    and alignment['validity'] == 'valid'
                    and alignment['authority_peer_id'] == clock['peer_id']
                    and product['peer_id'] == clock['peer_id']
                    and product['product_id'] == 'robot-to-scene-transform'
                    and bool(product['manifest_hash'])
                    and alignment['source_frame']['id'] == frame
                    and alignment['target_frame']['id'] == home_frame
                    and alignment['transform']['from_frame_id'] == frame
                    and alignment['transform']['to_frame_id'] == home_frame
                    and evidence['portal'] == portal
                    and evidence['clock'] == clock
                    and evidence['capture_timestamp_ns'] == e['capture_timestamp_ns'])
    except (KeyError, TypeError, ValueError):
        return False


def ease(t):
    t = np.clip(t, 0., 1.)
    return float(t*t*(3-2*t))
