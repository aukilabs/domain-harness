#!/usr/bin/env python3
"""galbot-g1-runner -- Flask runner for the Galbot G1.

What this serves:
  * Auth/infrastructure (this file's spine): DDS enrollment, DomainClient,
    and the always-on calibration loop.
  * The tasks (tasks/), behind a declarative registry, on the GalbotControl SDK.
  * A teleop-driven Auki capture session, and the teleop safety layer.

Startup order matters, and two things about it are deliberate:

1. FLASK STARTS BEFORE THE SDK. GalbotControl.__init__ takes 10-30s (four
   subsystem inits plus a settle), and the poc ran it before app.run() -- so
   /api/status could not answer during the window when you most want it, and a
   Ctrl-C in that window hit a half-built SDK. Flask now runs on a daemon
   thread and the main thread parks, so signals land somewhere predictable.

2. IDENTITY BOOTSTRAP IS ITS OWN THREAD. It talks to DDS and can block or
   fail without taking the HTTP surface, the tasks or teleop down with it.

   This file used to also run a libp2p cluster join here, publishing domain
   pose, joint encoders and a device model. That is gone: the Auki SDK
   removed its Python `auki_domain` and `auki_network` bindings (auki-sdk
   #360, and `chore(python): remove legacy Domain binding`), so there is
   nothing left for it to call. Capture and recording are unaffected -- they
   use `auki_registry`/`auki_layout`/`auki_logs`/`auki_manifests`, which all
   survive.

Run:
    python main.py            # binds 127.0.0.1 (localhost only), INFO
    python main.py --debug    # binds 0.0.0.0 (LAN reachable), DEBUG
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import signal
import sys
import threading
from uuid import uuid4

from flask import Flask, Response, jsonify, send_from_directory

import api_auth
import config
import routes
import tasks  # noqa: F401 -- imported for its registration side effects
import version
from calibration import CalibrationState, calibration_loop
from context import RunContext
from jobs import JobStore
from robot_lock import RobotLock
from session import CaptureSession
from utils.teleop import TeleopController
from utils.video import gen_mjpeg_frames
from utils import enrollment_info
from utils import logs
from utils import nav_tuning
from utils import shelf_tuning
from utils.logs import get_logger
from utils import signals

log = get_logger(__name__)

# How long the cleanup thread gets before we exit regardless. Short on purpose:
# past a few seconds Ctrl-C reads as a hang, and the one thing that must happen
# -- stopping the base -- is attempted first. systemd's TimeoutStopSec would
# SIGKILL eventually, but a self-imposed bound keeps an interactive Ctrl-C
# honest too.
SHUTDOWN_GRACE_S = 4.0

# Kernel-enforced ceiling on the entire shutdown. Must stay above
# SHUTDOWN_GRACE_S so the orderly path normally wins. This exists for one case:
# cleanup blocking inside a C call that holds the GIL, where no Python-level
# timer, join timeout or watchdog thread can run at all.
HARD_EXIT_S = 8

def _web_dir():
    """Where index.html / drive.html live.

    Frozen builds unpack `datas` into sys._MEIPASS, so the web/ directory is
    there rather than beside a source file that no longer exists.
    """
    if getattr(sys, "frozen", False) and hasattr(sys, "_MEIPASS"):
        return os.path.join(sys._MEIPASS, "web")
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), "web")


WEB_DIR = _web_dir()


class _FlagState:
    """Minimal thread-safe flag holder for enrollment progress."""

    def __init__(self, **fields):
        self._lock = threading.Lock()
        self._fields = dict(fields)

    def update(self, **fields):
        with self._lock:
            self._fields.update(fields)

    def snapshot(self):
        with self._lock:
            return dict(self._fields)


def parse_args(argv=None):
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--debug", action="store_true", default=None,
        help="bind 0.0.0.0 and log at DEBUG (default: 127.0.0.1 at INFO). "
             "Never enables Flask's own debugger/reloader -- the reloader would "
             "re-exec main() and double-start the background threads and the "
             "robot SDK session.")
    parser.add_argument("--host", default=None,
                        help="explicit bind host; overrides the --debug default")
    parser.add_argument("--port", type=int, default=None, help="bind port (default 5000)")
    parser.add_argument("--version", action="store_true",
                        help="print version/commit/build time as JSON and exit")
    return parser.parse_args(argv)


def build_app(cfg, ctx, jobs, session, teleop, calibration_state,
              enrollment_state):
    app = Flask(__name__, static_folder=None)
    # The capabilities route stays behind the guard by default. The escape
    # hatch is merged locally, never into api_auth.PUBLIC_PATHS.
    api_auth.install(app, cfg, extra_public_paths=(
        ("/api/capabilities",) if cfg.capabilities_public else ()))

    @app.route("/", methods=["GET"])
    def index():
        # The control panel.
        return send_from_directory(WEB_DIR, "index.html")

    @app.route("/drive", methods=["GET"])
    def drive():
        # The joystick cockpit. Separate page, not a tab: any scroll container
        # swallows the joystick drag.
        return send_from_directory(WEB_DIR, "drive.html")

    @app.route("/video_feed", methods=["GET"])
    def video_feed_default():
        return video_feed("head_left")

    @app.route("/video_feed/<cam_key>", methods=["GET"])
    def video_feed(cam_key):
        if not ctx.robot_state.snapshot()["ready"]:
            return jsonify({"success": False, "error": "robot is not ready"}), 503
        try:
            from utils.sdk import galbot_sensor_types
        except ImportError as exc:
            return jsonify({"success": False, "error": str(exc)}), 503
        if cam_key not in galbot_sensor_types:
            return jsonify({"success": False, "error": f"unknown camera {cam_key}"}), 404
        return Response(
            gen_mjpeg_frames(ctx.galbot, cam_key),
            mimetype="multipart/x-mixed-replace; boundary=frame")

    @app.route("/<path:filename>", methods=["GET"])
    def web_asset(filename):
        return send_from_directory(WEB_DIR, filename)

    # Shared and mutable: the capabilities route reads it per request, and the
    # node populates it when it starts. An external node therefore sees the
    # live feeds this robot is actually serving, not a list fixed at boot.
    import auki_runner_protocol
    resources = auki_runner_protocol.ResourceRegistry()
    wiring = routes.register_all(app, ctx, jobs, session, teleop,
                                 calibration_state, enrollment_state, cfg=cfg,
                                 resources=resources)
    return app, wiring


# ------------------------------------------------------------------ bootstrap

def _start_node(ctx, cfg, jobs, wiring, credentials):
    """Bring up the DMS worker and the Auki peer, or explain why not.

    Never raises. A robot that cannot claim DMS work still serves the Runner
    Protocol over HTTP, still drives under teleop, and still answers
    /api/status -- which is where the reason ends up.
    """
    if wiring is None or jobs is None:
        return
    wiring.registration = credentials
    try:
        import node
        started = node.start(cfg, ctx, jobs, wiring.gates, wiring.hooks,
                             credentials,
                             version=version.VERSION.get("version") or "0.0.0")
    except Exception as exc:  # noqa: BLE001
        log.error("robot node could not be started: %s", exc, exc_info=True)
        return
    wiring.node = started
    if started is not None:
        # The lease checks target_peer_id against this. The snapshot's
        # `capabilities` also carries the REWRITTEN /robot/teleop/v0/<peer_id>,
        # which is how an operator reads off the exact leaseable name.
        try:
            ctx.node_peer_id = (started.state.snapshot() or {}).get("peer_id")
        except Exception as exc:  # noqa: BLE001
            log.warning("could not read the node's peer id: %s", exc)
    if started is not None and wiring.resources is not None:
        # Advertise over HTTP exactly what the peer is serving.
        for live in started.resources():
            try:
                wiring.resources.register(live.resource)
            except Exception as exc:  # noqa: BLE001
                log.warning("could not advertise resource %s: %s",
                            live.resource_id, exc)
        model = started.device_model()
        if model is not None:
            wiring.resources.set_device_model(model)


def bootstrap_identity(ctx, cfg, calibration_state, enrollment_state, stop_event,
                       jobs=None, wiring=None):
    """Enroll, build the domain clients, and start the calibration loop.

    Retries with backoff and never raises: a DDS outage must not take down
    teleop, the tasks, or the HTTP surface.
    """
    import enrollment as enrollment_mod

    backoff = 2.0
    while not stop_event.is_set():
        try:
            enrollment_state.update(stage="enrolling")
            credentials, robot_name, client_id, dds_url, dms_url = \
                enrollment_mod.ensure_registration_credentials(
                    ctx.galbot, cfg.reg_secret_path, cfg.enrollment_info_overlay)
            enrollment_state.update(enrolled=True, stage="enrolled", error=None)

            from auki_domain_client_3_8 import DomainClient
            from auki_domain_navigation import DomainNavigation

            # client_id/dds_url from THIS scan/read, not cfg.domain_client_config's
            # baked-in values: on a first-ever boot, cfg.client_id was still
            # "unenrolled" and cfg.dds_url was still None at config.load() time
            # (before this scan resolved the real ones) -- this is the only
            # other place either feeds into anything (confirmed: config.py's
            # _validate() aside).
            domain_client_config = dict(
                cfg.domain_client_config, client_id=client_id, dds_base_url=dds_url)
            domain_client = DomainClient(dict(
                domain_client_config, registration_credentials=credentials))
            ctx.domain_client = domain_client

            # DomainNavigation first: it is what tasks plan paths with, and it
            # does not need portals, so tasks can navigate even if the
            # calibrator below is still failing to load them.
            navigation = DomainNavigation(domain_client, cfg.nav_config)
            ctx.navigation = navigation
            calibration_state.set_navigation(navigation)

            from auki_calibration import DomainCalibrator
            calibrator = DomainCalibrator(domain_client, cfg.calib_config)
            ctx.calibrator = calibrator
            enrollment_state.update(stage="ready", domain_id=domain_client.assigned_domain_id)

            log.info("identity ready: robot=%s domain=%s",
                     getattr(domain_client, "robot_id", "?"),
                     domain_client.assigned_domain_id)

            if os.environ.get('GALBOT_COMPONENT_MAPPING') != '1':
                calib_thread = threading.Thread(
                    target=calibration_loop,
                    args=(ctx.galbot, calibrator, calibration_state, stop_event),
                    kwargs=dict(poll_interval=1.0 / max(0.1, cfg.calib_hz),
                                portal_refresh_s=cfg.calib_portal_refresh_s),
                    daemon=True, name="calibration")
                calib_thread.start()

            # LAST, and only after calibration is running: the node advertises
            # a live pose feed, and a peer that appears before anything can
            # produce one is worse than a peer that appears a moment later.
            #
            # node_cfg, not cfg: node.py reads cfg.dds_url/cfg.dms_url, both
            # baked at config.load() time -- still None on a first-ever boot,
            # before the scan above resolved them. Without this the node stays
            # blocked ("no DDS URL") until the NEXT restart re-read the
            # overlay, on the one boot that just did the work to unblock it.
            #
            # cfg.dms_url wins when set because config.py resolved it as
            # DMS_BASE_URL-then-overlay: the environment must still override
            # what the QR carried. cfg.dds_url is not consulted -- it has no
            # env source at all, so the freshly resolved value is always the
            # better one.
            node_cfg = dataclasses.replace(
                cfg, dds_url=dds_url, dms_url=cfg.dms_url or dms_url)
            _start_node(ctx, node_cfg, jobs, wiring, credentials)
            return
        except Exception as exc:  # noqa: BLE001
            enrollment_state.update(stage="retrying", error=str(exc))
            log.warning("identity bootstrap failed (%s); retrying in %.0fs", exc, backoff)
            if stop_event.wait(backoff):
                return
            backoff = min(backoff * 2, 60.0)


# ----------------------------------------------------------------------- main

def bootstrap_robot(ctx, cfg, calibration_state, enrollment_state,
                    robot_lock, teleop_holder, stop_event,
                    jobs=None, wiring=None):
    """Bring up the robot SDK and everything that depends on it.

    ON A WORKER THREAD, deliberately. GalbotControl() blocks for 10-30s inside
    C++, and while the main thread sits in a call like that it cannot run a
    Python signal handler -- Ctrl-C did nothing at all for the whole window.

    More importantly, the SDK installs its own SIGINT handler during bring-up,
    which replaces ours process-wide (signal dispositions are per-process, not
    per-thread). Reclaiming it needs signal.signal(), which ONLY works on the
    main thread -- so the main thread has to be free to keep re-asserting it.
    See _park().

    Everything here is best-effort: a failure leaves robot_state carrying the
    reason and the HTTP surface answering 503, rather than killing the process.
    """
    # --- SDK bring-up (blocking, reported via /api/status) ---
    ctx.robot_state.set_stage("sdk-init")
    try:
        from utils.sdk import GalbotControl, GalbotInitError
    except ImportError as exc:
        ctx.robot_state.mark_error(f"galbot_sdk unavailable: {exc}", stage="no-sdk")
        log.error("galbot_sdk unavailable: %s", exc)
        log.warning("serving anyway: /healthz and /api/tasks work; robot routes return 503")
        return

    try:
        ctx.galbot = GalbotControl(
            debug=cfg.debug,
            sensors=cfg.galbot_sensors,
            enable_motion=cfg.enable_motion,
            enable_navigation=cfg.enable_navigation,
            enable_perception=cfg.enable_perception,
            init_settle_s=cfg.init_settle_s,
            home_on_init=False,   # never in a constructor; see below
        )
    except GalbotInitError as exc:
        ctx.robot_state.mark_error(str(exc), stage="sdk-failed")
        log.error("robot init failed: %s", exc)
        log.warning("serving anyway so /api/status can report it")
        return

    teleop = TeleopController(
        ctx.galbot,
        raw_velocity=cfg.teleop_raw_velocity,
        max_linear_mps=cfg.teleop_max_linear_mps,
        max_yaw_radps=cfg.teleop_max_yaw_radps,
        watchdog_timeout_s=cfg.teleop_watchdog_s,
        max_smooth_dt_s=cfg.teleop_max_smooth_dt_s,
        linear_accel=cfg.teleop_linear_accel, linear_decel=cfg.teleop_linear_decel,
        yaw_accel=cfg.teleop_yaw_accel, yaw_decel=cfg.teleop_yaw_decel,
        owner_check=lambda: robot_lock.owner()[0],
    )
    teleop_holder["teleop"] = teleop
    # Tasks reach the controller through the context (the lease watches for a
    # local disable), and the hold is what a teleop_lease job owns.
    ctx.teleop = teleop
    ctx.remote_teleop_max_lease_s = cfg.remote_teleop_max_lease_s
    if cfg.remote_teleop_enabled:
        from utils.remote_teleop import RemoteTeleopHold, make_head_mover
        ctx.remote_teleop = RemoteTeleopHold(
            teleop,
            require_freshness=True,
            deadman_s=cfg.remote_teleop_deadman_s,
            idle_end_s=cfg.remote_teleop_idle_end_s,
            tick_s=cfg.remote_teleop_tick_s,
            max_linear_mps=cfg.remote_teleop_max_linear_mps,
            max_yaw_radps=cfg.remote_teleop_max_yaw_radps,
            require_sender=cfg.remote_teleop_require_sender,
            head=make_head_mover(ctx.galbot),
            smooth=cfg.remote_teleop_smooth,
        )
        log.info("remote teleop enabled (max lease %.0fs, require_sender=%s)",
                 cfg.remote_teleop_max_lease_s, cfg.remote_teleop_require_sender)
    # When SOMEONE ELSE takes the base, zero the smoother so teleop cannot later
    # resume from a stale ramp.
    #
    # The `new_owner != "teleop"` guard is essential, not defensive. /api/velocity
    # acquires and releases the lock around every single command, so the lock is
    # free again by the next one -- which made every teleop acquire look like a
    # change of owner and fire this callback. reset_smoother() sets
    # _last_smooth_time=None, so the next _smooth() saw dt=0.0, and
    # _slew_toward(0, target, accel, 0.0) returns 0.0. Teleop sent (0,0,0) on
    # every command and the base never moved at all.
    robot_lock.on_preempt(
        lambda new_owner: None if new_owner == "teleop"
        else teleop.reset_smoother(0.0, 0.0, 0.0))
    teleop.start_watchdog()

    ctx.esl_model = _load_esl_model(cfg)
    ctx.robot_state.mark_ready()

    threading.Thread(target=bootstrap_identity,
                     args=(ctx, cfg, calibration_state, enrollment_state, stop_event),
                     kwargs=dict(jobs=jobs, wiring=wiring),
                     daemon=True, name="bootstrap-identity").start()

    if cfg.home_on_start:
        log.warning("GALBOT_HOME_ON_START is set: homing now (THIS MOVES THE ROBOT)")
        with robot_lock.acquire("home", timeout=5.0):
            ctx.galbot.home()



def main(argv=None):
    args = parse_args(argv)

    if args.version:
        print(json.dumps(version.VERSION))
        return 0

    # FIRST, before anything that can block: guarantee Ctrl-C works.
    #
    # A signal.signal() handler only runs on the main thread at a bytecode
    # boundary, so it is useless while the main thread sits inside the 10-30s
    # C++ SDK bring-up below, or inside a shutdown call that never returns. The
    # guard is a separate thread woken by CPython's C-level handler, so it can
    # act when the main thread cannot. See utils/signals.py.
    shutting_down = threading.Event()
    signals.force_exit_on_signal(shutting_down,
                                 cleanup_grace_s=SHUTDOWN_GRACE_S + 2.0)

    # A placeholder handler until there is state worth unwinding. Nothing is
    # built yet, so os._exit IS the orderly path -- and installing it now means
    # there is no window where SIGINT would dump a traceback instead.
    def _early_shutdown(signum=None, frame=None):
        shutting_down.set()
        os.write(2, b"\ninterrupted during startup\n")
        os._exit(signals.EXIT_INTERRUPTED)

    signal.signal(signal.SIGINT, _early_shutdown)
    signal.signal(signal.SIGTERM, _early_shutdown)

    # Configure logging before anything can fail: a bad .env is exactly when you
    # need the message to land somewhere.
    logs.configure("DEBUG" if args.debug else "INFO")

    try:
        cfg = config.load({"host": args.host, "port": args.port, "debug": args.debug})
    except config.ConfigError as exc:
        log.error("Configuration error:\n%s", exc)
        return 2

    # Re-configure at the level the config asked for. `logfile=` is the ONE place
    # a log file gets wired in -- see utils/logs.configure().
    logs.configure(cfg.log_level, logfile=None, force=True)
    nav_tuning.configure(cfg.nav_tuning)
    shelf_tuning.configure(cfg.shelf_tuning)
    log.info("nav tuning: %s", cfg.nav_tuning)
    log.info("shelf tuning: %s", cfg.shelf_tuning)

    calibration_state = CalibrationState()
    enrollment_state = _FlagState(enrolled=False, stage="pending", error=None, domain_id=None)
    robot_lock = RobotLock()
    stop_event = threading.Event()

    ctx = RunContext(galbot=None, calibration_state=calibration_state, cfg=cfg,
                     robot_lock=robot_lock, results_root=cfg.results_dir)

    session_holder = {}
    jobs = JobStore(
        busy_check=lambda: (
            session_holder["session"].busy_reason() if "session" in session_holder else None),
        log_dir=cfg.job_log_dir,
    )

    teleop_holder = {}
    session = CaptureSession(ctx, _teleop_capture_runner)
    session_holder["session"] = session

    app, wiring = build_app(cfg, ctx, jobs, session, _LazyTeleop(teleop_holder),
                            calibration_state, enrollment_state)

    # Flask FIRST, on a daemon thread, so /api/status can answer during the
    # 10-30s SDK bring-up and signals land on the parked main thread.
    if cfg.binds_publicly and not cfg.api_token:
        log.warning(
            "Bound to %s with no API_TOKEN -- any host on this network can drive "
            "the robot. Set API_TOKEN to require a bearer token.", cfg.host)
    log.info("serving on http://%s:%d  (%s)", cfg.host, cfg.port,
             "DEBUG, network-reachable" if cfg.binds_publicly else "localhost only")
    server = threading.Thread(
        target=lambda: app.run(host=cfg.host, port=cfg.port, debug=False,
                               use_reloader=False, threaded=True),
        daemon=True, name="flask")
    server.start()

    def _shutdown(signum=None, frame=None):
        # A second signal means the cleanup below is stuck -- go straight out.
        # os.write, not logging: a forced-exit path must not be able to block on
        # a lock it does not need.
        if shutting_down.is_set():
            os.write(2, b"second signal; exiting immediately\n")
            os._exit(0)
        shutting_down.set()

        # Acknowledge before ANY lock or logging call, so Ctrl-C always shows
        # something immediately even if the console is busy.
        os.write(2, b"\nstopping...\n")
        log.info("stopping...")

        # Every call in the cleanup below can block without bound:
        # galbot.shutdown() ends in an untimed robot.wait_for_shutdown(), and the
        # domain close is a network call. So run the WHOLE sequence on a
        # worker thread and join it with a deadline, rather than hoping each step
        # returns. Whatever finished, finished; then we go.
        #
        # SHUTDOWN_GRACE_S is deliberately short. A shutdown that takes eight
        # seconds is indistinguishable from a hang to whoever pressed Ctrl-C, and
        # the only thing that truly matters -- stopping the base -- happens in the
        # first few calls.
        stop_event.set()

        # THE ONLY BOUND THAT SURVIVES A HELD GIL.
        #
        # Measured on the robot: the SDK teardown could block inside a pybind11
        # call holding the GIL, starving every Python thread -- the worker join
        # below, the signal guard, all of it. Nothing written in Python can fire
        # in that state, which is why the earlier timer and guard-thread
        # approaches both failed.
        #
        # SIGALRM is left at its default disposition, so delivery AND termination
        # are performed by the kernel with zero Python execution. Cancelled below
        # if cleanup finishes in time; otherwise the process dies (exit 142).
        signal.alarm(HARD_EXIT_S)

        def _cleanup():
            # The node comes down FIRST, and before the robot SDK. It is the
            # only thing here with a remote counterpart: an unmounted endpoint
            # and a closed lease are how a consumer and DMS learn this robot is
            # going away, rather than timing out. It also stops a task claiming
            # the base while the rest of this function is stopping it.
            node_handle = wiring.node if wiring else None
            if node_handle is not None:
                try:
                    node_handle.stop(timeout=SHUTDOWN_GRACE_S)
                except Exception:  # noqa: BLE001
                    pass
            try:
                session.stop()
            except Exception:  # noqa: BLE001
                pass
            teleop = teleop_holder.get("teleop")
            if teleop is not None:
                try:
                    teleop.send_hard_stop()
                    teleop.stop_watchdog(timeout=0.5)
                except Exception:  # noqa: BLE001
                    pass
            mapping = getattr(wiring, "mapping", None)
            if mapping is not None:
                try:
                    mapping.stop(timeout=0.5)
                except Exception:  # noqa: BLE001
                    pass
            obj = getattr(ctx, "domain_client", None)
            if obj is not None:
                try:
                    obj.close()
                except Exception:  # noqa: BLE001
                    pass
            if ctx.galbot is not None:
                try:
                    ctx.galbot.shutdown()
                except Exception:  # noqa: BLE001
                    pass
        worker = threading.Thread(target=_cleanup, daemon=True, name="shutdown")
        worker.start()
        worker.join(timeout=SHUTDOWN_GRACE_S)
        if worker.is_alive():
            os.write(2, b"cleanup did not finish; exiting anyway\n")

        # The Galbot SDK does not exit cleanly; a hard exit is the only reliable way.
        signal.alarm(0)
        os._exit(0)

    signal.signal(signal.SIGINT, _shutdown)
    signal.signal(signal.SIGTERM, _shutdown)

    # Bring the robot up on a WORKER thread so the main thread stays free to
    # handle signals -- see bootstrap_robot() and _park().
    robot_args = (ctx, cfg, calibration_state, enrollment_state,
                  robot_lock, teleop_holder, stop_event, jobs, wiring)
    if cfg.sdk_init_on_main_thread:
        # Escape hatch: if the SDK's own Python layer calls signal.signal()
        # during construction it raises "signal only works in main thread" off
        # the main thread, and bring-up would fail outright. This trades Ctrl-C
        # responsiveness during bring-up for getting the robot up at all.
        log.warning("GALBOT_SDK_INIT_ON_MAIN_THREAD is set: Ctrl-C will not "
                    "respond during the 10-30s bring-up")
        bootstrap_robot(*robot_args)
    else:
        threading.Thread(target=bootstrap_robot, args=robot_args,
                         daemon=True, name="bootstrap-robot").start()

    return _park(stop_event, _shutdown)


def _park(stop_event, handler=None):
    """Keep the main thread alive, and keep OWNERSHIP of the signal handlers.

    Two jobs. The obvious one is staying interruptible -- a short interval bounds
    any platform where a long wait is less responsive to signals.

    The other is re-asserting `handler` every pass. The robot SDK installs its
    own SIGINT handler while it initialises, which replaces ours for the whole
    PROCESS -- signal dispositions are not per-thread, so running bring-up on a
    worker thread does not prevent it. The symptom is Ctrl-C stopping the robot
    but never exiting, because the SDK's handler ran instead of ours.

    signal.signal() only works on the main thread, so this loop is the only place
    that can take it back. That is the whole reason bring-up moved off the main
    thread. Re-installing an already-installed handler is a cheap no-op, so doing
    it a few times a second costs nothing and closes the window whenever the
    theft happens.
    """
    try:
        while not stop_event.wait(0.2):
            if handler is not None:
                _claim_signals(handler)
    except KeyboardInterrupt:
        pass
    return 0


def _claim_signals(handler):
    """(Re-)install our handlers. Main thread only; never raises.

    UNCONDITIONALLY, without consulting signal.getsignal() first. That check
    looks like an obvious optimisation and silently breaks the whole mechanism:
    getsignal() returns the handler PYTHON last recorded, not the disposition the
    kernel actually holds. A C++ SDK calling sigaction()/signal() directly is
    invisible to it, so the check would report "still ours" and skip the reinstall
    in exactly the case this exists for.

    signal.signal() is a cheap syscall, so re-installing a few times a second
    costs nothing measurable.
    """
    for signum in (signal.SIGINT, signal.SIGTERM):
        try:
            signal.signal(signum, handler)
        except (OSError, ValueError):
            pass


def _load_esl_model(cfg):
    """Load the YOLO ESL weights, or None. Never fatal.

    Only detect_esl / shelf_scan / inspect / demo need it; teleop and capture
    do not, and refusing to start without weights would be a bad trade.
    """
    path = cfg.esl_model
    if not path or not os.path.isfile(path):
        log.warning("ESL model not found at %s; ESL tasks will return 503", path)
        return None
    try:
        from ultralytics import YOLO
        model = YOLO(path)
        model(verbose=False)   # warm up the graph before the first task needs it
        log.info("ESL model loaded: %s", path)
        return model
    except Exception as exc:  # noqa: BLE001
        log.warning("failed to load ESL model %s: %s", path, exc)
        return None


def _teleop_capture_runner(galbot, **kwargs):
    """Indirection so session.py does not import a robot-only module at startup."""
    from tasks.capture_teleop import run_capture_auki
    return run_capture_auki(galbot, **kwargs)


class _LazyTeleop:
    """Stands in for the TeleopController until the SDK is up.

    Routes are registered before the SDK exists (deliberately), but they must
    not blow up if someone posts a velocity during bring-up -- they should get
    the same 503 the robot_state check produces.
    """

    def __init__(self, holder):
        self._holder = holder

    def __getattr__(self, name):
        teleop = self._holder.get("teleop")
        if teleop is None:
            raise RuntimeError("teleop is not available until the robot SDK is up")
        return getattr(teleop, name)

    def last_velocity(self):
        teleop = self._holder.get("teleop")
        return teleop.last_velocity() if teleop else None

    # Overridden (not left to __getattr__) so GET /api/status can report teleop
    # during bring-up instead of raising. A pre-SDK controller is, correctly,
    # not enabled.
    def state(self):
        teleop = self._holder.get("teleop")
        if teleop is None:
            return {"enabled": False, "disabled_reason": "robot SDK is not up yet"}
        return teleop.state()

    def is_enabled(self):
        teleop = self._holder.get("teleop")
        return bool(teleop.is_enabled()) if teleop else False

    # Disarming is a risk-reducing action, so /api/teleop/disable is
    # deliberately NOT gated on robot-ready -- which means it can be called
    # before this holder is filled. "Nothing to disable" is a success, not a 500.
    def disable(self, reason="operator"):
        teleop = self._holder.get("teleop")
        if teleop is None:
            return {"enabled": False, "reason": reason,
                    "was_rolling": False, "stopped": False}
        return teleop.disable(reason=reason)

    @property
    def watchdog_timeout_s(self):
        teleop = self._holder.get("teleop")
        return getattr(teleop, "watchdog_timeout_s", None) if teleop else None


if __name__ == "__main__":
    sys.exit(main())
