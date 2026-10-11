# Bracketbot Android reaching behavior

Implement a Bracketbot behavior that reaches toward a calibrated Android phone using their shared Home coordinates.

Requirements:

- Begin only when both devices have successfully calibrated into the same Home scene and have fresh, valid tracking. Bind the target to the selected Android’s peer identity.
- Do not try to move the base.
- Select the arm whose shoulder is nearest the Android. Add hysteresis so small movements around the centerline do not repeatedly switch arms.
- Choose a new pointing/reaching target at **0.5 Hz—once every two seconds**.
- Extend the selected arm toward the Android, aiming the fingers toward it. Target a configurable stand-off distance, initially **20 cm**, accounting for the whole gripper. If the target is beyond reach, find the closest safe reachable pose.
- Execute smooth, bounded trajectories using the robot’s kinematics, measured joint limits, and collision checks. Keep the other arm neutral. Retract the active arm before switching arms.
- Run motor control and safety checks continuously at higher rates than target selection.
- If the phone disconnects, loses tracking/calibration, or its pose becomes stale, promptly stop reaching and **gracefully return the arm to a defined neutral pose**. Resume only after valid tracking has stabilized. If safe retraction is impossible, stop and report the reason.
- Coordinate with the existing Portal-search stance so only one behavior owns arm control. Preserve BBOS protections and controller priority rules.

Inspect the existing Android pose publication, Home calibration evidence, and Bracketbot control code first. Use explicit SDK coordinate transforms between Home, the robot navigation frame, and the arm frame; do not derive commands from the displayed robot glyph. Handle capture clocks, session resets, delayed messages, and calibration changes explicitly.

Before implementing, describe the architecture, neutral pose, freshness thresholds, arm-switching rules, and validation plan. Identify missing calibration or obstacle-sensing capabilities without inventing them.

Validate with unit tests, simulated targets, and read-only planning before bounded physical testing. Report what was verified and any remaining blockers. Reuse the **single existing Home server at `http://127.0.0.1:4329/`**; do not open another. The robot is Bracketbot 0184 at `192.168.1.118`, accessible through `bb ssh 0184`.
