"""Fail-closed policy for portal search followed by phone pointing.

No motor writes here. A valid alignment survives loss of QR visibility: its
validity belongs to the mapper, not to a timeout on the last QR observation.
"""
from bracketbot_pointing import fresh, inverse_point


def supervisor_fresh(heartbeat, controller_pid, parent_pid, now_ns):
    try:
        return (controller_pid == parent_pid == heartbeat['pid']
                and fresh((now_ns-heartbeat['updated_ns'])/1e6, 999))
    except (KeyError, TypeError, ValueError):
        return False


def alignment_valid(scene, home_frame):
    try:
        owner, frame = scene['authority_peer_id'], scene['frame']
        r = scene['registries']; m = r['local_mapping']
        a, product, robot = r['frame_transform'], r['frame_transform_product'], m['robot_pose']
        e = a['evidence']; measured = m['observed_portals'][e['portal']]
        inverse_point(a['transform'], (0, 0, 0))  # finite, unit rigid transform
        return bool(a['schema'] == 'spatial.frame-transform/v1'
            and a['validity'] == 'valid' and a['authority_peer_id'] == owner
            and product['peer_id'] == owner and product['manifest_hash']
            and product['product_id'] == 'robot-to-scene-transform'
            and a['source_frame']['id'] == frame
            and a['target_frame']['id'] == home_frame
            and a['transform']['from_frame_id'] == frame
            and a['transform']['to_frame_id'] == home_frame
            and robot['frame'] == frame and robot['clock']['peer_id'] == owner
            and e['clock'] == measured['clock'] == robot['clock']
            and e['capture_timestamp_ns'] == measured['capture_timestamp_ns'])
    except (KeyError, TypeError, ValueError, OverflowError):
        return False


def decide(scene, home_frame, *, scene_age_ms, robot_pose_age_ms,
           runner_alive, fault=None):
    result = dict(actuation_enabled=False, search_requested=False)
    if fault:
        return dict(result, phase='fault', reason=fault)
    if not runner_alive:
        return dict(result, phase='waiting_for_tracking', reason='Runner unavailable/stale')
    if not fresh(scene_age_ms,500):
        return dict(result,phase='waiting_for_tracking',reason='Waiting for current runner control state')
    try:
        if alignment_valid(scene, home_frame):
            if (not fresh(scene_age_ms, 500) or not fresh(robot_pose_age_ms, 800)
                    or scene['registries']['local_mapping']['status'] != 'mapping'):
                return dict(result, phase='waiting_for_tracking', phone_requested=True,
                            reason='Aligned robot pose unavailable/stale')
            return dict(result, phase='pointing_blocked',
                        phone_requested=True, reason='Phone reach requires explicit execution enable')
        # Fixed wrist search uses live joint feedback and measured limits, not
        # a world-space target. It must work while SLAM is lost/unlocalized.
        return dict(result, phase='searching', search_requested=True,
                    reason='No valid transform into Home')
    except (KeyError, TypeError):
        return dict(result, phase='waiting_for_tracking', reason='Invalid mapper state')


class SearchLatch:
    """Do not automatically retry a failed motion or fight a manual override."""
    def __init__(self):
        self.fault = None

    def completed(self, report, *, cancelled):
        if report.get('phase') == 'already_calibrated':
            return
        if (report.get('phase') != 'restored'
                or report.get('reason') not in ('calibrated', 'cancelled')
                or (report.get('reason') == 'cancelled' and not cancelled)):
            self.fault = 'Search controller stopped unexpectedly; inspect and explicitly rearm'
