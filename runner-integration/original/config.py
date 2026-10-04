"""Central configuration: one dotenv load, one frozen Config, typed env readers.

Replaces three different config mechanisms that used to coexist here:
python-dotenv plus argparse over os.environ; a second hand-rolled dotenv loader
in the shelf-layout fetch path; and a /userdata config.yaml. (The last two are
gone entirely -- that fetch path became utils/fixture_layout.py, which does no
I/O at all.)

Three loaders with different override semantics meant behaviour depended on
which module imported first. There is now exactly one `load_dotenv()`, called
from `load()`, called once from main().

CRITICAL: this module owns every value the app reads, but it must NOT hide the
environment variables that robot-runner-kit reads from `os.environ` *itself*
(CLIENT_ID, DDS_BASE_URL, CAMERA_MATRIX, DIST_COEFFS, ROBOT_RADIUS,
DSC_CACHE_DIR, DSC_CACHE_REVALIDATE_SECONDS). For those we build explicit
`domain_client_config` / `nav_config` / `calib_config` dicts and pass them to
the library constructors, so .env load order can never change behaviour.

Path accessors are FUNCTIONS, not module constants, so importing this module
early can't bake in a stale value before load_dotenv() has run.

Deliberately absent: any config.yaml path, and any calibration-artifact path.
Calibration is an always-on background loop with no disk persistence, and
`domain_id` comes live from
DomainClient.assigned_domain_id. See calibration.py and the README.
"""

from __future__ import annotations

import dataclasses
import json
import os
import sys
from pathlib import Path
from utils.enrollment_info import load_overlay as _load_enrollment_info_overlay
from utils.nav_tuning import NavTuning
from utils.shelf_tuning import ShelfTuning
from utils.logs import get_logger

log = get_logger(__name__)


def _bundle_root() -> Path:
    """Directory holding the app's own read-only data files.

    In a PyInstaller onefile build the source tree does not exist on disk: the
    binary unpacks into a temp dir exposed as sys._MEIPASS, and that is where
    files listed in the spec's `datas` land. robot_params.yaml is read from
    here, so it MUST be in `datas` -- otherwise a frozen build has no speed
    defaults and falls back to FALLBACK_DEFAULTS with a warning.
    """
    if getattr(sys, "frozen", False) and hasattr(sys, "_MEIPASS"):
        return Path(sys._MEIPASS)
    return Path(__file__).resolve().parent


REPO_ROOT = _bundle_root()

_TRUE = {"1", "true", "yes", "on", "y", "t"}
_FALSE = {"0", "false", "no", "off", "n", "f"}


# --------------------------------------------------------------------------
# typed env readers -- one place, so coercion bugs have one home
# --------------------------------------------------------------------------

def _s(name: str, default: str | None = None) -> str | None:
    raw = os.environ.get(name)
    if raw is None:
        return default
    raw = raw.strip()
    return raw if raw else default


def _i(name: str, default: int) -> int:
    raw = _s(name)
    if raw is None:
        return default
    try:
        return int(raw)
    except ValueError:
        raise ConfigError(f"{name} must be an integer, got {raw!r}")


def _f(name: str, default: float) -> float:
    raw = _s(name)
    if raw is None:
        return default
    try:
        return float(raw)
    except ValueError:
        raise ConfigError(f"{name} must be a number, got {raw!r}")


def _b(name: str, default: bool) -> bool:
    raw = _s(name)
    if raw is None:
        return default
    low = raw.lower()
    if low in _TRUE:
        return True
    if low in _FALSE:
        return False
    raise ConfigError(f"{name} must be a boolean (1/0, true/false), got {raw!r}")


def _list(name: str, default: tuple[str, ...]) -> tuple[str, ...]:
    raw = _s(name)
    if raw is None:
        return tuple(default)
    return tuple(part.strip() for part in raw.split(",") if part.strip())


def _json(name: str, default):
    raw = _s(name)
    if raw is None:
        return default
    try:
        return json.loads(raw)
    except ValueError as exc:
        raise ConfigError(f"{name} must be valid JSON: {exc}")


class ConfigError(RuntimeError):
    """Raised for a malformed or missing required setting.

    main() catches this and exits with the message, so a bad .env produces one
    readable line instead of a traceback from wherever the value was first used.
    """


# --------------------------------------------------------------------------
# path defaults
# --------------------------------------------------------------------------

# Everything the app writes lives under one root.
#
# /var/lib/<pkg> is where Debian policy puts variable state, and it composes with
# systemd: the unit declares `StateDirectory=auki/galbot-g1-runner`, so systemd
# creates and chowns this on every start, and it stays writable even under
# ProtectSystem=strict (which would remount /opt read-only).
#
# The `galbot-g1-runner` component is NOT redundant here, unlike under /opt: a
# robot image may carry other packages, and /var/lib/auki itself stays
# root-owned so nothing else can write into this tree. This STATE_DIR holds
# only per-app, per-package state -- robot_params.local.yaml and data/.
# Robot-identity and model files live one level up; see ROBOT_STATE_DIR and
# MODELS_SHARED_DIR below.
STATE_DIR = "/var/lib/auki/galbot-g1-runner"

# reg_secret is the ROBOT's credential, not this package's.
# It is scoped to the physical robot rather than this one app so that, if
# another app is ever deployed alongside this one on the same robot, it can
# reuse the same enrollment/identity instead of minting its own -- two runner
# packages on one robot should mean one DDS registration and one peer_id, not
# two. So unlike STATE_DIR, this is NOT under this package's own subtree and is
# NOT a systemd StateDirectory= (systemd would chown it recursively to *this*
# unit's User:Group on every start, fighting any other unit for ownership). It
# is provisioned once by postinst: root:auki, mode 2770 (setgid so files
# created by any service user in the group stay group `auki`), and the unit
# joins that group via SupplementaryGroups=auki.
ROBOT_STATE_DIR = "/var/lib/auki/robot"

# Model weights (currently just esl.pt) are large, not secret, and not specific
# to this package either -- if another app on the same robot ever needs the
# same weights, it should read this copy rather than fetching its own ~100MB.
# Same postinst provisioning as ROBOT_STATE_DIR, but mode 2775: world-readable
# since there is nothing sensitive in a weights file, group-writable so any app
# in the group can update it.
MODELS_SHARED_DIR = "/var/lib/auki/models"

# Used only when the matching root above is not writable -- see
# _writable_root(). Nested under one dev root so a workstation checkout collects
# everything in one place instead of three.
_DEV_STATE_DIR = Path.home() / ".local" / "state" / "galbot_g1_runner"
_DEV_ROBOT_STATE_DIR = _DEV_STATE_DIR / "robot"
_DEV_MODELS_DIR = _DEV_STATE_DIR / "models"

# The path accessors below are functions and get called repeatedly (each one
# re-derives its root), so each fallback warning is latched to fire once per
# process rather than once per lookup.
_fallback_warned: set[str] = set()


def _writable_root(root: str, dev_fallback: Path, what: str, set_var: str) -> str:
    """Writable root, or an XDG-ish path on a dev machine.

    The fallback exists so `python main.py` works where /var/lib is not writable
    (most dev machines), keeping the off-robot checks in the README runnable. It
    is LOUD rather than silent: a robot that cannot write here would otherwise
    quietly persist state somewhere nobody looks.
    """
    if _is_writable_dir(root):
        return root
    if root not in _fallback_warned:
        _fallback_warned.add(root)
        log.warning(
            "%s is not writable; falling back to %s for %s. On a robot this is a "
            "permissions problem -- create %s and make it writable, or set %s.",
            root, dev_fallback, what, root, set_var,
        )
    return str(dev_fallback)


def default_state_dir() -> str:
    """Writable per-package state root: STATE_DIR, or a dev fallback.

    Set GALBOT_STATE_DIR explicitly and this is not consulted.
    """
    return _writable_root(STATE_DIR, _DEV_STATE_DIR, "robot params and task output",
                           "GALBOT_STATE_DIR")


def default_robot_state_dir() -> str:
    """Writable robot-identity root: ROBOT_STATE_DIR, or a dev fallback.

    Set REG_SECRET_PATH explicitly and this is not consulted for that value.
    """
    return _writable_root(ROBOT_STATE_DIR, _DEV_ROBOT_STATE_DIR,
                           "the enrollment secret",
                           "REG_SECRET_PATH")


def default_models_dir() -> str:
    """Writable shared-models root: MODELS_SHARED_DIR, or a dev fallback.

    Set GALBOT_MODELS_DIR explicitly and this is not consulted.
    """
    return _writable_root(MODELS_SHARED_DIR, _DEV_MODELS_DIR, "model weights",
                           "GALBOT_MODELS_DIR")


def _is_writable_dir(path: str) -> bool:
    """True if `path` exists and is writable, or can be created."""
    if os.path.isdir(path):
        return os.access(path, os.W_OK | os.X_OK)
    parent = os.path.dirname(path.rstrip("/")) or "/"
    return os.path.isdir(parent) and os.access(parent, os.W_OK | os.X_OK)


# --------------------------------------------------------------------------
# Function-style path accessors.
#
# These are functions, not module constants, so that importing this module
# before load_dotenv() has run cannot bake in a stale value. Modules that need
# a single path without holding a whole Config (utils/robot_params.py) call
# these; load() below uses them too, so there is exactly one definition of each
# default.
# --------------------------------------------------------------------------

def state_dir_path() -> str:
    # Check the env FIRST and only then compute the default. `_s(name,
    # default_state_dir())` would evaluate the default eagerly -- and
    # default_state_dir() has a side effect (the not-writable warning), so an
    # operator who had correctly set GALBOT_STATE_DIR still got warned about
    # STATE_DIR that the app was never going to touch.
    explicit = _s("GALBOT_STATE_DIR")
    return explicit if explicit else default_state_dir()


def data_dir_path() -> str:
    return _s("GALBOT_DATA_DIR", os.path.join(state_dir_path(), "data"))


def results_dir_path() -> str:
    return _s("GALBOT_RESULTS_DIR", os.path.join(data_dir_path(), "results"))


def models_dir_path() -> str:
    # Check the env first, same reasoning as state_dir_path(): default_models_dir()
    # has a side-effecting warning, which an operator who set GALBOT_MODELS_DIR
    # explicitly should never see.
    explicit = _s("GALBOT_MODELS_DIR")
    return explicit if explicit else default_models_dir()


def esl_model_path() -> str:
    return _s("GALBOT_ESL_MODEL", os.path.join(models_dir_path(), "esl.pt"))


def robot_params_defaults_path() -> str:
    """Checked-in defaults, at the repo root next to main.py."""
    return str(REPO_ROOT / "robot_params.yaml")


def robot_params_overlay_path() -> str:
    """Control-panel overlay. Outside the source tree, so a read-only deploy works."""
    return _s(
        "GALBOT_ROBOT_PARAMS_OVERLAY",
        os.path.join(state_dir_path(), "robot_params.local.yaml"),
    )


def enrollment_info_overlay_path() -> str:
    """robot_name/client_id/dds_url, resolved from the enrollment QR and
    persisted here by enrollment.py -- see utils/enrollment_info.py.

    Lives under ROBOT_STATE_DIR (/var/lib/auki/robot), not this package's own
    STATE_DIR: these three values come from the SAME enrollment ceremony as
    reg_secret (same QR, same redeem() call) and identify the
    physical robot, not this package -- a sibling runner package sharing
    reg_secret must see the same robot_name/client_id too, or two apps on one
    robot would present two different identities for the one DDS
    registration they both share. Only the FILE PATH is env-relocatable, not
    the values themselves (there is no ROBOT_NAME/DDS_URL env fallback
    anymore -- QR/overlay only).
    """
    # Check the env first, same reasoning as state_dir_path():
    # default_robot_state_dir() has a side-effecting warning, which an
    # operator who set GALBOT_ENROLLMENT_INFO_OVERLAY explicitly should never see.
    explicit = _s("GALBOT_ENROLLMENT_INFO_OVERLAY")
    return explicit if explicit else os.path.join(
        default_robot_state_dir(), "enrollment_info.local.yaml")


def job_log_dir_path() -> str:
    """Directory for per-job log files. One file per job, written by JobStore."""
    return _s("GALBOT_JOB_LOG_DIR", os.path.join(data_dir_path(), "logs", "jobs"))


def capture_root_path() -> str:
    return _s("AUKI_CAPTURE_ROOT", os.path.join(data_dir_path(), "captures"))


# --------------------------------------------------------------------------
# Config
# --------------------------------------------------------------------------

@dataclasses.dataclass(frozen=True)
class Config:
    # --- serving / auth ---
    host: str
    port: int
    debug: bool
    log_level: str
    api_token: str | None
    api_cors_origins: tuple[str, ...]
    # Escape hatch for a deployment that cannot give the local robot-node an
    # API_TOKEN: serve GET /api/capabilities unauthenticated, in a REDACTED
    # form carrying only what the node needs to dispatch. Off by default --
    # the full document leaks robot model, instance id, task names, parameter
    # names and gate details, which is exactly what the pre-merge /api/status
    # leaked and why this guard exists at all.
    capabilities_public: bool

    # --- paths ---
    state_dir: str
    data_dir: str
    results_dir: str
    models_dir: str
    esl_model: str
    robot_params_overlay: str
    enrollment_info_overlay: str
    reg_secret_path: str
    capture_root: str
    job_log_dir: str

    # --- DDS / enrollment identity ---
    dds_url: str | None
    robot_name: str
    client_id: str

    # --- robot node (DMS worker + Auki peer, in this process) ---
    #
    # NONE OF THIS IS REQUIRED, and that is deliberate: a robot that has not
    # been given them keeps working exactly as before, serving the Runner
    # Protocol over HTTP for an external node. The node starts only when it has
    # everything it needs, and /api/status says what is missing when it does
    # not -- so rolling this out is a deployment change, not a flag day.
    #
    # `robot_audience` is an OVERRIDE, not a required setting, since auki-sdk
    # added robot-audience defaults: the three official Auki DDS endpoints
    # derive `<dds_url>/robots`
    # in the SDK, and a custom or path-modified DDS URL has no default and
    # fails closed with "provide an explicit robot audience for custom DDS
    # endpoints". Left None here, the SDK decides. Set, it is used verbatim and
    # never falls back to a preset -- the SDK compares it against the token's
    # `aud` exactly -- so a stale value outranks a correct default rather than
    # quietly talking to the wrong audience.
    node_enabled: bool
    dms_url: str | None
    robot_audience: str | None
    #: This robot's peer identity. Lives beside reg_secret in the SHARED robot
    #: state dir for the same reason reg_secret does: two runner packages on one
    #: robot should mean one registration and one peer id, not two. One live
    #: process may own a given key.
    peer_identity_file: str
    #: True: the peer publishes its own reachability so other peers can find
    #: it. False: it reads others' advertisements and stays invisible itself.
    peer_advertise: bool
    #: Content-addressed store for task artifacts, the URDF and its meshes.
    #: One root, because Blob v1 is mounted once per process and serves all of
    #: it; the registry entries live here too.
    node_app_root: str
    #: Where a blob_zip_dir artifact is staged before it becomes a blob.
    #: Configurable rather than /tmp because a recording can be gigabytes.
    zip_staging_dir: str

    # --- calibration (poc-style loop; no gates, no persistence) ---
    calib_hz: float
    calib_sensors: tuple[str, ...]
    calib_method: str
    calib_portal_refresh_s: float

    # --- domain navigation (robot-runner-kit reads most of these itself) ---
    robot_radius: float
    #: This app's own nav thresholds (GALBOT_NAV_*); main() hands it to
    #: utils.nav_tuning.configure(). See utils/nav_tuning.py for each field.
    nav_tuning: NavTuning
    #: Inspect wrist-camera standoff and shelf base depths (GALBOT_SHELF_*);
    #: main() hands it to utils.shelf_tuning.configure().
    shelf_tuning: ShelfTuning

    # --- robot / SDK ---
    galbot_sensors: tuple[str, ...] | None
    enable_motion: bool
    enable_navigation: bool
    enable_perception: bool
    home_on_start: bool
    init_settle_s: float
    sdk_init_on_main_thread: bool
    min_battery_pct: float
    abandon_battery_pct: float

    # --- teleop ---
    teleop_max_linear_mps: float
    teleop_max_yaw_radps: float
    teleop_linear_accel: float
    teleop_linear_decel: float
    teleop_yaw_accel: float
    teleop_yaw_decel: float
    teleop_max_smooth_dt_s: float
    teleop_watchdog_s: float
    teleop_raw_velocity: bool

    # --- remote teleop over DMS (/robot/teleop/v0) ---
    remote_teleop_enabled: bool
    remote_teleop_require_sender: bool
    remote_teleop_max_lease_s: float
    remote_teleop_deadman_s: float
    remote_teleop_idle_end_s: float
    remote_teleop_smooth: bool
    remote_teleop_tick_s: float
    remote_teleop_queue: int
    remote_teleop_max_linear_mps: float
    remote_teleop_max_yaw_radps: float

    # --- library config dicts (explicit, never implicit os.environ) ---
    domain_client_config: dict
    nav_config: dict
    calib_config: dict

    # ---------------------------------------------------------------- helpers
    @property
    def binds_publicly(self) -> bool:
        return self.host not in ("127.0.0.1", "localhost", "::1")


def _tuning_from_env(cls, prefix: str, positive: tuple[str, ...] = ()):
    """Build a frozen tuning dataclass from `prefix + FIELD_NAME.upper()` env vars.

    Unset vars keep the class defaults. Every value must be a number >= 0;
    fields named in `positive` must be > 0.
    """
    defaults = cls()
    values = {}
    for field in dataclasses.fields(cls):
        env = prefix + field.name.upper()
        value = _f(env, getattr(defaults, field.name))
        if field.name in positive and value <= 0:
            raise ConfigError(f"{env} must be > 0, got {value}")
        if value < 0:
            raise ConfigError(f"{env} must be >= 0, got {value}")
        values[field.name] = value
    return cls(**values)


def _nav_tuning() -> NavTuning:
    """GALBOT_NAV_* -> NavTuning.

    A zero safety factor or stall window would time out or "stall" every leg
    instantly; the distances may be 0 (= feature off).
    """
    return _tuning_from_env(
        NavTuning, "GALBOT_NAV_", positive=("timeout_safety_factor", "stall_s"))


def _shelf_tuning() -> ShelfTuning:
    """GALBOT_SHELF_* -> ShelfTuning. All distances must be > 0: a zero camera
    standoff puts the camera in the product, a zero work depth the base in the
    shelf."""
    return _tuning_from_env(
        ShelfTuning, "GALBOT_SHELF_",
        positive=tuple(f.name for f in dataclasses.fields(ShelfTuning)))


def load(overrides: dict | None = None) -> Config:
    """Read .env + environment into a frozen Config. Call once, from main().

    `overrides` carries resolved CLI flags (argparse) that beat the
    environment -- e.g. {"host": ..., "port": ..., "debug": ...}.
    """
    from dotenv import load_dotenv

    load_dotenv()
    overrides = overrides or {}

    debug = overrides.get("debug")
    if debug is None:
        debug = _b("DEBUG", False)

    # An explicit --host / $HOST always wins. Otherwise: localhost-only by
    # default, and 0.0.0.0 in debug mode. This is the merge's main security
    # change -- all three source repos defaulted to 0.0.0.0 with no auth.
    host = overrides.get("host") or _s("HOST") or ("0.0.0.0" if debug else "127.0.0.1")
    port = overrides.get("port") or _i("PORT", 5000)

    state_dir = state_dir_path()
    data_dir = data_dir_path()
    results_dir = results_dir_path()
    models_dir = models_dir_path()
    capture_root = capture_root_path()

    # Check the env first, same reasoning as state_dir_path(): default_robot_state_dir()
    # has a side-effecting warning, which an operator who set REG_SECRET_PATH
    # explicitly should never see.
    reg_secret_path = _s("REG_SECRET_PATH") or os.path.join(
        default_robot_state_dir(), "reg_secret")

    # robot_name/client_id/dds_url come from the enrollment
    # QR now, not env -- see enrollment.py / utils/enrollment_info.py.
    # "unenrolled" is only ever seen before the very first QR scan completes
    # on a truly fresh robot; an already-enrolled robot reads its real values
    # back from this overlay (or, for a robot enrolled under the old
    # .env-based flow before this overlay existed, enrollment.py migrates
    # robot_name/client_id from reg_secret's embedded robot_id the first time
    # it runs post-upgrade -- dds_url is not recoverable
    # that way and stay empty for that one robot until it's re-enrolled).
    enrollment_info_overlay = enrollment_info_overlay_path()
    _enrollment_info = _load_enrollment_info_overlay(enrollment_info_overlay)
    robot_name = _enrollment_info.get("robot_name") or "unenrolled"
    client_id = _enrollment_info.get("client_id") or robot_name
    dds_url = _enrollment_info.get("dds_url")
    # Env first, overlay second. The audience and the DMS URL are deployment
    # facts rather than per-robot enrollment facts, so an operator sets them in
    # the unit's environment the way every other worker does; the overlay is
    # there for a deployment whose enrollment QR carries them.
    dms_url = _s("DMS_BASE_URL") or _enrollment_info.get("dms_url")
    robot_audience = _s("DDS_ROBOT_AUDIENCE") or _enrollment_info.get("robot_audience")
    peer_identity_file = _s("AUKI_PEER_IDENTITY_FILE") or os.path.join(
        default_robot_state_dir(), "peer.identity")
    node_app_root = _s("AUKI_NODE_APP_ROOT") or os.path.join(data_dir, "auki")
    zip_staging_dir = _s("AUKI_ZIP_STAGING_DIR") or os.path.join(data_dir, "staging")

    cfg = Config(
        host=host,
        port=port,
        debug=bool(debug),
        log_level=_s("LOG_LEVEL", "DEBUG" if debug else "INFO"),
        api_token=_s("API_TOKEN"),
        api_cors_origins=_list("API_CORS_ORIGINS", ()),
        capabilities_public=_b("GALBOT_CAPABILITIES_PUBLIC", False),

        state_dir=state_dir,
        data_dir=data_dir,
        results_dir=results_dir,
        models_dir=models_dir,
        esl_model=esl_model_path(),
        robot_params_overlay=robot_params_overlay_path(),
        enrollment_info_overlay=enrollment_info_overlay,
        reg_secret_path=reg_secret_path,

        node_enabled=_b("GALBOT_NODE_ENABLED", True),
        dms_url=dms_url,
        robot_audience=robot_audience,
        peer_identity_file=peer_identity_file,
        # Default True. The DDS tracker is the SDK's only discovery source, so
        # a robot that does not advertise cannot be found by anything, however
        # healthy its endpoints are -- that is a deliberate choice, not a
        # default worth inheriting by accident.
        peer_advertise=_b("AUKI_PEER_ADVERTISE", True),
        node_app_root=node_app_root,
        zip_staging_dir=zip_staging_dir,
        capture_root=capture_root,
        job_log_dir=job_log_dir_path(),

        dds_url=dds_url,
        robot_name=robot_name,
        client_id=client_id,

        calib_hz=_f("GALBOT_CALIB_HZ", 2.0),
        calib_sensors=_list("GALBOT_CALIB_SENSORS", ("left_arm", "right_arm")),
        calib_method=_s("GALBOT_CALIB_METHOD", "auki"),
        calib_portal_refresh_s=_f("GALBOT_CALIB_PORTAL_REFRESH_S", 300.0),

        robot_radius=_f("ROBOT_RADIUS", 0.4),
        nav_tuning=_nav_tuning(),
        shelf_tuning=_shelf_tuning(),

        galbot_sensors=_list("GALBOT_SENSORS", ()) or None,
        enable_motion=_b("GALBOT_ENABLE_MOTION", True),
        enable_navigation=_b("GALBOT_ENABLE_NAVIGATION", True),
        enable_perception=_b("GALBOT_ENABLE_PERCEPTION", True),
        home_on_start=_b("GALBOT_HOME_ON_START", False),
        init_settle_s=_f("GALBOT_INIT_SETTLE_S", 3.0),
        # Escape hatch. Bring-up normally runs on a worker thread so the main
        # thread stays free to handle signals; set this if the SDK's own Python
        # layer calls signal.signal() during construction, which raises "signal
        # only works in main thread" off the main thread. Costs Ctrl-C
        # responsiveness for the 10-30s of bring-up.
        sdk_init_on_main_thread=_b("GALBOT_SDK_INIT_ON_MAIN_THREAD", False),
        min_battery_pct=_f("GALBOT_MIN_BATTERY_PCT", 30.0),
        abandon_battery_pct=_f("GALBOT_ABANDON_BATTERY_PCT", 25.0),

        teleop_max_linear_mps=_f("TELEOP_MAX_LINEAR_MPS", 0.3),
        teleop_max_yaw_radps=_f("TELEOP_MAX_YAW_RADPS", 0.6),
        teleop_linear_accel=_f("TELEOP_LINEAR_ACCEL", 0.6),
        teleop_linear_decel=_f("TELEOP_LINEAR_DECEL", 1.5),
        teleop_yaw_accel=_f("TELEOP_YAW_ACCEL", 1.2),
        teleop_yaw_decel=_f("TELEOP_YAW_DECEL", 2.5),
        teleop_max_smooth_dt_s=_f("TELEOP_MAX_SMOOTH_DT_S", 0.3),
        teleop_watchdog_s=_f("TELEOP_WATCHDOG_S", 0.5),
        # Default True preserves the tuned teleop feel
        # exactly: raw robot.set_base_velocity, no collision check, the
        # server-side watchdog as the only deadman. See utils/teleop.py.
        teleop_raw_velocity=_b("TELEOP_RAW_VELOCITY", True),

        # On by default so the peer advertises Message and Peyote shows Teleop.
        # Booster has no such switch. `0` still drops the channel, the hold,
        # the Info snapshot, and the peer-identity requirement.
        remote_teleop_enabled=_b("REMOTE_TELEOP_ENABLED", True),
        # FAIL CLOSED. The SDK puts the driver at event["sender"]["peer_id"].
        # With this on and no transport sender, every drive message is refused
        # and the lease degrades to a hold that stops the robot -- which is the
        # right way to be wrong. Setting 0 accepts a self-asserted
        # driver_peer_id from the payload, which ANY peer that can reach the
        # endpoint could forge; bench bring-up only.
        remote_teleop_require_sender=_b("REMOTE_TELEOP_REQUIRE_SENDER", True),
        # Under the catalog's own 900 s ceiling. A site can raise it.
        remote_teleop_max_lease_s=_f("REMOTE_TELEOP_MAX_LEASE_S", 300.0),
        # Mirrors TELEOP_WATCHDOG_S. The lease runs its OWN deadman because
        # RobotLock's job:<id> owner silences TeleopController's -- see
        # utils/remote_teleop.py.
        remote_teleop_deadman_s=_f("REMOTE_TELEOP_DEADMAN_S", 0.5),
        # A dropped driver stops the base within deadman_s. Optionally (>0)
        # also end the lease after this long idle; 0 keeps it until its
        # deadline or a DMS cancel, as booster-k1-runner does.
        remote_teleop_idle_end_s=_f("REMOTE_TELEOP_IDLE_END_S", 0.0),
        # 1 routes remote commands through the cockpit's slew ramp. Off by
        # default: on top of link latency it made the base answer late.
        remote_teleop_smooth=_b("REMOTE_TELEOP_SMOOTH", False),
        remote_teleop_tick_s=_f("REMOTE_TELEOP_TICK_S", 0.05),
        remote_teleop_queue=_i("REMOTE_TELEOP_QUEUE", 16),
        # Independently lowerable: a driver who is not in the room may be held
        # to less than the cockpit. The hold clamps to min(remote, teleop).
        remote_teleop_max_linear_mps=_f("REMOTE_TELEOP_MAX_LINEAR_MPS",
                                        _f("TELEOP_MAX_LINEAR_MPS", 0.3)),
        remote_teleop_max_yaw_radps=_f("REMOTE_TELEOP_MAX_YAW_RADPS",
                                       _f("TELEOP_MAX_YAW_RADPS", 0.6)),

        # dds_base_url is dds_url -- no longer a separate DDS_BASE_URL env var
        # (it was always the same DDS instance; see enrollment_info_overlay_path()).
        # Passed explicitly here (not left to auki_domain_client_3_8's own
        # os.environ fallback -- see the module docstring) so .env load order
        # can never change behaviour, same reasoning as client_id below.
        domain_client_config={
            "dds_base_url": dds_url,
            "client_id": client_id,
        },
        nav_config=_nav_config(),
        calib_config=_calib_config(),
    )

    _validate(cfg)
    _make_dirs(cfg)
    return cfg


def _nav_config() -> dict:
    """Explicit dict for auki_domain_navigation.DomainNavigation.

    Only include keys that were actually set, so the library's own defaults
    still apply for the rest.

    The ctor reads exactly three keys -- robot_radius, cache_dir,
    revalidate_seconds -- and nothing else. Five *_endpoint keys used to be
    built here (MAP_ENDPOINT, PATH_ENDPOINT, OPTIMIZED_PATH_ENDPOINT,
    OPTIMIZED_SEGMENT_PATH_ENDPOINT, RESTRICTED_DEST_ENDPOINT); the kit had
    already stopped reading them before 0.3.0, so they were dead on arrival
    and are gone. There is no HTTP spatial service left to point them at:
    since 0.4.0 the kit fetches the domain's OBJs and bakes/queries them
    in-process through the auki-sdk Rust bindings.

    ROBOT_RADIUS changed MEANING in 0.4.0 even though its name did not. It is
    now a post-bake landmass query radius (BakeProfile.dsc), and bake-time
    Recast erosion is 0 -- so it no longer carves a clearance corridor the way
    the old eroded bake did. Re-validate it on the robot; do not assume the
    0.4 tuned against the old behaviour still means the same thing.
    """
    out: dict = {"robot_radius": _f("ROBOT_RADIUS", 0.4)}
    # Kit defaults: ~/.cache/robot-runner-kit/domains and 300s. Set
    # DSC_CACHE_DIR to the systemd StateDirectory on a packaged install, so a
    # User=galbot service is not caching meshes under $HOME.
    cache_dir = _s("DSC_CACHE_DIR")
    if cache_dir:
        out["cache_dir"] = cache_dir
    revalidate = _s("DSC_CACHE_REVALIDATE_SECONDS")
    if revalidate:
        out["revalidate_seconds"] = _f("DSC_CACHE_REVALIDATE_SECONDS", 300.0)
    return out


def _calib_config() -> dict:
    """Explicit dict for auki_calibration.DomainCalibrator.

    camera_matrix is deliberately NOT set here: intrinsics come live from the
    SDK per camera, and DomainCalibrator.detect_and_calibrate() mutates a
    caller-supplied matrix in place (it scales it without copying first), so a
    single shared matrix would be corrupted frame by frame. See calibration.py.
    """
    out: dict = {}
    dist = _json("DIST_COEFFS", None)
    if dist is not None:
        out["dist_coeffs"] = dist
    return out


def _validate(cfg: Config) -> None:
    """Collect every problem, then raise once.

    The predecessor exited separately for each required setting in turn, so a
    fresh deploy hit the errors one restart at a time. Nothing is required
    now: DDS_URL/ROBOT_NAME/CLIENT_ID/AUKI_DISCOVERY_URL are QR-derived, and
    AUKI_APP_ROOT went with cluster publishing.
    """
    problems: list[str] = []

    # No CLIENT_ID/DDS_URL required-checks here anymore: both are QR-derived
    # (robot_name/client_id default to "unenrolled" pre-scan, never empty;
    # dds_url is resolved fresh per-scan inside enrollment.py, not pre-known
    # at config.load() time at all -- see utils/enrollment_info.py).

    if (cfg.min_battery_pct > 0 and cfg.abandon_battery_pct > 0
            and cfg.abandon_battery_pct > cfg.min_battery_pct):
        problems.append(
            f"GALBOT_ABANDON_BATTERY_PCT ({cfg.abandon_battery_pct:.0f}) is above "
            f"GALBOT_MIN_BATTERY_PCT ({cfg.min_battery_pct:.0f}). A job would be "
            "allowed to start and then abandoned immediately -- abandon must be at "
            "or below the start floor."
        )

    if cfg.remote_teleop_enabled:
        if not (1.0 <= cfg.remote_teleop_max_lease_s <= 900.0):
            problems.append(
                f"REMOTE_TELEOP_MAX_LEASE_S ({cfg.remote_teleop_max_lease_s:g}) is "
                "outside 1..900. The /robot/teleop/v0 catalog entry clamps "
                "duration_ms to 1s..15min, so a value outside that can never "
                "take effect."
            )
        if (cfg.remote_teleop_idle_end_s > 0
                and cfg.remote_teleop_deadman_s >= cfg.remote_teleop_idle_end_s):
            problems.append(
                f"REMOTE_TELEOP_DEADMAN_S ({cfg.remote_teleop_deadman_s:g}) is at or "
                f"above REMOTE_TELEOP_IDLE_END_S ({cfg.remote_teleop_idle_end_s:g}). "
                "The lease would end before its deadman ever stopped the base."
            )
        if cfg.remote_teleop_tick_s > cfg.remote_teleop_deadman_s / 2.0:
            problems.append(
                f"REMOTE_TELEOP_TICK_S ({cfg.remote_teleop_tick_s:g}) is more than half "
                f"REMOTE_TELEOP_DEADMAN_S ({cfg.remote_teleop_deadman_s:g}). The deadman "
                "is evaluated on the tick, so it could not fire on time."
            )

    if problems:
        raise ConfigError(
            "Configuration problems (see .env.example):\n  - " + "\n  - ".join(problems)
        )


def _is_within(child: str, parent: str) -> bool:
    try:
        return os.path.commonpath([child, parent]) == parent
    except ValueError:  # different drives / unrelated roots
        return False


def _make_dirs(cfg: Config) -> None:
    # reg_secret's directory is deliberately NOT one of cfg's own
    # dir fields (it is ROBOT_STATE_DIR, shared with sibling runner packages,
    # not this package's own state_dir) -- so it isn't covered by the loop
    # below unless added via its parent explicitly. On a robot this is a no-op
    # (postinst already created it with root:auki ownership); on a dev machine
    # this is what actually creates the fallback dir under ~/.local/state/.
    dirs = {cfg.state_dir, cfg.data_dir, cfg.results_dir, cfg.models_dir,
            os.path.dirname(cfg.reg_secret_path)}
    for path in dirs:
        try:
            os.makedirs(path, exist_ok=True)
        except OSError as exc:
            raise ConfigError(f"cannot create {path}: {exc}")


def describe(cfg: Config) -> dict:
    """Config as a dict for logging / a debug endpoint, secrets redacted."""
    secret_fields = {"api_token"}
    out = {}
    for field in dataclasses.fields(cfg):
        value = getattr(cfg, field.name)
        if field.name in secret_fields:
            value = "<set>" if value else None
        elif field.name == "domain_client_config":
            value = {k: v for k, v in value.items() if k != "registration_credentials"}
        out[field.name] = value
    return out
