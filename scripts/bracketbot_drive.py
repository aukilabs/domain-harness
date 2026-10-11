"""Fail-closed joystick policy for the Bracketbot base. No motor writes here.

Input is the runner's `drive-command.json`: Home's latest stick command for
this robot, already filtered to the one phone the launcher allowed. This module
decides the twist. Anything missing, stale, malformed or from another phone is
a stop.
"""
import math

# The limits bbapps/teleop.py uses for held keys: forward on both wheels, and
# the per-wheel difference for a turn in place. Wheels never exceed MAX_WHEEL.
MAX_FORWARD_MPS = 0.20
MAX_TURN_WHEEL_MPS = 0.15
MAX_WHEEL_MPS = 0.28
# Total age budget: Home dwell, request round trip, the runner's cache interval
# and this controller's own loop. Measured on robot 0184, one read from Home
# takes 130-390 ms, so 400 ms discarded nearly every command. This bound only
# governs a lost link: a released stick stops through explicit zero commands,
# and at half speed one second is about 10 cm.
MAX_INPUT_AGE_MS = 1000
DEADZONE = 0.08
STOP = (0.0, 0.0)


def input_age_ms(packet, now_ns):
    """Age of the stick reading, summing every hop without comparing clocks
    across machines: Home's dwell, the request round trip, and local time since
    the runner cached it."""
    command = packet['command']
    local_ms = (now_ns - int(packet['received_ns'])) / 1e6
    if local_ms < 0:
        raise ValueError('cache is from the future')
    return local_ms + float(command['age_ms']) + float(packet['request_age_ms'])


def shaped(value):
    """Deadzone, then rescale so motion starts from zero at its edge."""
    value = float(value)
    if not math.isfinite(value) or abs(value) > 1:
        raise ValueError('stick out of range')
    if abs(value) < DEADZONE:
        return 0.0
    return math.copysign((abs(value) - DEADZONE) / (1 - DEADZONE), value)


def twist(packet, *, allowed_peer, now_ns, half_width_m, scale=0.5):
    """Return ((v_mps, w_radps), reason). Reason is None only when driving."""
    try:
        if not (0 < scale <= 1) or not (0.05 < half_width_m < 1):
            return STOP, 'invalid controller limits'
        if packet.get('schema') != 'spatial.robot-drive-input/v1':
            return STOP, 'unknown input schema'
        if packet.get('authenticated') is not True:
            return STOP, 'Home unreachable'
        command = packet.get('command')
        if command is None:
            return STOP, 'no command'
        if not allowed_peer or command['driver_peer_id'] != allowed_peer \
                or packet.get('allowed_driver_peer_id') != allowed_peer:
            return STOP, 'command is not from the allowed phone'
        age = input_age_ms(packet, now_ns)
        if not age <= MAX_INPUT_AGE_MS:
            return STOP, f'command stale ({age:.0f} ms)'
        forward, turn = shaped(command['forward']), shaped(command['turn'])
    except (KeyError, TypeError, ValueError, OverflowError) as error:
        return STOP, f'invalid input: {error}'
    if forward == 0 and turn == 0:
        return STOP, 'stick centered'
    v = forward * MAX_FORWARD_MPS * scale
    # Positive turn is counter-clockwise: the right wheel runs faster.
    difference = turn * MAX_TURN_WHEEL_MPS * scale
    fastest = max(abs(v - difference), abs(v + difference))
    limit = MAX_WHEEL_MPS * scale
    if fastest > limit:
        v, difference = v * limit / fastest, difference * limit / fastest
    return (v, difference / half_width_m), None
