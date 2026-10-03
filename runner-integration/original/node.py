"""Start this robot's DMS worker and Auki peer inside the runner process.

WHY THIS IS OPTIONAL AND SILENT ABOUT IT. Everything the node needs beyond what
the app already has -- a DMS URL, the native wheels, and on a non-Auki DDS the
deployment's exclusive robot audience -- is a deployment fact, not a robot fact. A robot that has
not been given them keeps working exactly as it did: the Runner Protocol is
still served over HTTP, and an external node can still drive it. So a missing
prerequisite is reported on /api/status and logged once, never raised. Rolling
this out is a deployment change, not a flag day.

The node runs the SAME tasks the HTTP routes run, through the same registry,
gates, JobStore and hooks. It is a second door into that machinery, not a
parallel implementation of it.
"""

from __future__ import annotations

import os
import re
import time

import tasks
from utils.logs import get_logger

log = get_logger(__name__)

#: A semver core: 1-3 dot-separated numbers, optionally followed by a
#: prerelease (`-`) and/or build metadata (`+`). Mirrors what DDS accepts in
#: pkg/utils.ParseSemanticVersion, which cuts everything from the first `+` or
#: `-` and requires the remainder to be numeric.
_SEMVER_CORE = re.compile(r"^[0-9]+(\.[0-9]+){0,2}([-+].*)?$")

#: What a version becomes when it is not semver. `0.0.0` registers and sorts
#: below every real release, which is the honest answer for a build that could
#: not say what it is.
_FALLBACK_VERSION = "0.0.0"


def semver_or_fallback(version):
    """Return `version` if DDS would accept it, else `0.0.0` -- and say so.

    THE BUG THIS EXISTS FOR. DDS validates the registration's version with
    ParseSemanticVersion and answers HTTP 400 on anything else; auki-sdk maps
    every non-2xx from that endpoint onto TaskError::Authentication, whose
    message is "machine authentication or registration failed". So a build
    stamped `dev` produced a robot that could never register, reporting an
    authentication failure -- sending you to audiences, credentials and DDS
    config, none of which were wrong.

    Coerce rather than refuse: a dev build with an unhelpful version should
    still run. Warn loudly, because reporting 0.0.0 for an unknown build is a
    small lie and the operator should be able to see it in the log.
    """
    text = (version or "").strip()
    if _SEMVER_CORE.match(text):
        return text
    log.warning(
        "version %r is not semver; DDS would reject registration with HTTP 400 "
        "(reported as an authentication failure). Registering as %s instead -- "
        "build with PACKAGE_VERSION set, or tag galbot-g1-runner/v<x.y.z>.",
        version, _FALLBACK_VERSION)
    return _FALLBACK_VERSION

#: Imported lazily and reported rather than raised. They are built from the
#: auki-sdk repository with maturin and are not on PyPI, so a robot that has
#: not been given them is an ordinary deployment state.
REQUIRED_WHEELS = ("auki_sdk", "auki_registry", "auki_datatypes")


def missing_wheels():
    missing = []
    for name in REQUIRED_WHEELS:
        try:
            __import__(name)
        except ImportError:
            missing.append(name)
    return missing


def _audience_required(dds_url):
    """Whether this deployment has to supply DDS_ROBOT_AUDIENCE itself.

    Asked of the kit rather than restated here, so one list of official DDS
    endpoints serves both and cannot drift. The kit is pure Python -- the
    native wheels are the part that may be absent -- but it is still imported
    lazily, because this module promises to import on a machine without the
    node extra.
    """
    try:
        from auki_robot_node import audience_required
    except ImportError:
        # No kit means no node at all, which `start()` already reports. A
        # second reason here would be one the operator cannot act on.
        return False
    return audience_required(dds_url)


def blockers(cfg, registration=None):
    """Everything preventing the node from starting, in operator-facing words."""
    reasons = []
    if not cfg.node_enabled:
        return ["GALBOT_NODE_ENABLED=0"]
    if not registration:
        reasons.append("the robot is not enrolled yet (no registration credential)")
    if not cfg.dds_url:
        reasons.append("no DDS URL (enrol the robot, or set DDS_BASE_URL)")
    if not cfg.dms_url:
        reasons.append("DMS_BASE_URL is not set")
    # Only for a DDS the SDK has no preset for. Since the SDK's robot-
    # audience defaults landed the
    # three official endpoints derive `<dds_url>/robots` themselves, so
    # demanding DDS_ROBOT_AUDIENCE there would block a robot that is correctly
    # configured. A custom or path-modified DDS still has no default, and the
    # SDK's own error arrives only at startup, so it is worth saying here.
    if not cfg.robot_audience and _audience_required(cfg.dds_url):
        reasons.append("DDS_ROBOT_AUDIENCE is not set, and {} is not one of "
                       "the DDS endpoints auki-sdk has a default audience "
                       "for; it must come from the DDS deployment "
                       "configuration".format(cfg.dds_url))
    if not tasks.REGISTRY.capability_index():
        reasons.append("no task declares a capability, so there would be "
                       "nothing to register")
    # Declaring /robot/teleop/v0 makes a resolvable peer identity MANDATORY:
    # the kit registers /robot/teleop/v0/<peer_id>, reading the id out of the
    # identity file BEFORE it builds the credential, and RAISES in _bring_up if
    # it cannot. That raise costs the robot ALL DMS work, not just teleop --
    # node.start() swallows it and returns None -- so it is worth saying here,
    # in operator words, instead of letting bring-up fail.
    if cfg.remote_teleop_enabled:
        if not cfg.peer_identity_file:
            reasons.append("remote teleop needs AUKI_PEER_IDENTITY_FILE: the "
                           "capability registered is /robot/teleop/v0/<peer_id> "
                           "and the peer id is read from that file before "
                           "registration")
        elif not _identity_dir_writable(cfg.peer_identity_file):
            reasons.append("cannot write the peer identity file at {}; without "
                           "it the node cannot name its teleop capability and "
                           "NO capability registers at all"
                           .format(cfg.peer_identity_file))
    missing = missing_wheels()
    if missing:
        reasons.append("missing native wheels: {}".format(", ".join(missing)))
    return reasons


def _identity_dir_writable(path):
    """Can we create/replace the peer identity file at `path`?"""
    try:
        if os.path.exists(path):
            return os.access(path, os.R_OK | os.W_OK)
        return os.access(os.path.dirname(path) or ".", os.W_OK)
    except Exception:  # noqa: BLE001
        return False


def _teleop_info_node(RobotNode, ctx, cfg, version):
    """RobotNode that also serves Info, with the teleop holder on app_instance.

    Peyote only shows Teleop when Message is advertised, then checks the grant
    on Info. Info v1 drops a raw `teleop` object, so the snapshot is packed
    behind `|teleop=`. Mounted only while remote teleop is on.
    """
    from utils import remote_teleop

    class GalbotRobotNode(RobotNode):
        def _mount(self, sdk):
            super()._mount(sdk)
            if self._peer is None or getattr(self, "_written", None) is None:
                return
            written = self._written
            peer = self._peer
            app_version = semver_or_fallback(version)
            device_name = getattr(cfg, "robot_name", None)

            def info_provider(requester):
                del requester
                clock = getattr(written, "clock", None) or {}
                hold = getattr(ctx, "remote_teleop", None)
                if hold is None:
                    snap = {"state": "idle", "holder_peer_id": None,
                            "expires_at_ns": None}
                else:
                    snap = hold.info_snapshot()
                return {
                    "app": "galbot-g1-runner",
                    "app_version": app_version,
                    "name": device_name,
                    "session_id": getattr(written, "session_id", None),
                    "session_clock_id": clock.get("id"),
                    "session_clock_hash": clock.get("hash"),
                    "session_now_ns": time.time_ns(),
                    "peer_id": peer.peer_id,
                    "teleop": snap,
                    "app_instance": remote_teleop.encode_app_instance("", snap),
                }

            self._endpoints.append(sdk.AukiInfoEndpoint.mount(peer, info_provider))
            log.info("mounted info on peer %s", peer.peer_id)

    return GalbotRobotNode


def build(cfg, ctx, jobs, gates, hooks, registration, version="0.0.0"):
    """Wire a `RobotNode` onto this app's existing machinery.

    Imported here rather than at module scope so the app still imports on a
    machine without the kit's node extra.
    """
    from auki_robot_node import (DISCOVER_AND_ADVERTISE, DISCOVER_ONLY,
                                 Dispatcher, MessageChannel, NodeConfig,
                                 RobotNode)

    import node_resources
    from utils import remote_teleop

    for directory in (cfg.node_app_root, cfg.zip_staging_dir):
        os.makedirs(directory, exist_ok=True)

    resources, device_model = node_resources.build(ctx)

    # The drive-message queue for a held teleop lease. The channel itself has
    # NO lease -- any authenticated peer can push into it at any time -- so the
    # hold is what makes a message legitimate: it refuses everything while no
    # lease is held, and refuses any sender that is not the leased driver.
    #
    # The callback reads ctx.remote_teleop PER CALL rather than capturing it:
    # this runs at build time, which can be before bootstrap_robot creates the
    # hold, and on a failed SDK bring-up it is never created at all.
    #
    # NOTE the thread: the kit drains this on its own asyncio loop, the same
    # one that polls DMS and delivers cancels. The handler must never block.
    channels = []
    if cfg.remote_teleop_enabled and os.environ.get("GALBOT_ODOMETRY_COMPONENT") != "1":
        channels.append(MessageChannel(
            remote_teleop.CONTROL_RESOURCE_ID,
            remote_teleop.make_on_message(lambda: getattr(ctx, "remote_teleop", None)),
            capacity=cfg.remote_teleop_queue))

    node_cls = (_teleop_info_node(RobotNode, ctx, cfg, version)
                if cfg.remote_teleop_enabled else RobotNode)
    if os.environ.get("GALBOT_ODOMETRY_COMPONENT") == "1":
        if os.environ.get("GALBOT_PORTAL_MAPPING_CONFIG"):
            raise ValueError("standalone odometry and portal mapping currently require separate runs")
        from odometry import odometry_node_class
        node_cls = odometry_node_class(node_cls, ctx)
    if os.environ.get("GALBOT_PORTAL_MAPPING_CONFIG"):
        from portal_mapping import mapping_node_class
        node_cls = mapping_node_class(node_cls, ctx)
    node = node_cls(
        config=NodeConfig(
            dds_url=cfg.dds_url, dms_url=cfg.dms_url,
            registration=registration, audience=cfg.robot_audience,
            client_id=cfg.client_id, version=semver_or_fallback(version),
            peer_identity_file=cfg.peer_identity_file,
            # The kit speaks the SDK's vocabulary; this app exposes the plain
            # operator question -- should this robot be findable or not.
            discovery_mode=(DISCOVER_AND_ADVERTISE if cfg.peer_advertise
                            else DISCOVER_ONLY)),
        resources=resources, device_model=device_model,
        message_channels=(channels or None),
        app_root=cfg.node_app_root, logger=log)

    # The dispatcher publishes against the peer the node holds, and the node
    # registers the capabilities the dispatcher exposes, so the two are built
    # as a pair and joined.
    import auki_registry

    return node.attach_dispatcher(Dispatcher(
        ctx=ctx, jobs=jobs, registry=tasks.REGISTRY, gates=gates, hooks=hooks,
        uploader_factory=node.uploader,
        blob_store=lambda data: auki_registry.put_blob(cfg.node_app_root, data),
        peer_info=node.peer_info,
        zip_staging_dir=cfg.zip_staging_dir,
        logger=log))


def start(cfg, ctx, jobs, gates, hooks, registration, version="0.0.0"):
    """Build and start the node, or explain why it did not start.

    Returns the `RobotNode` on success and None otherwise. Never raises: a
    robot that cannot claim DMS work is degraded, not broken.
    """
    reasons = blockers(cfg, registration)
    if reasons:
        log.info("robot node not started: %s", "; ".join(reasons))
        return None
    node = None
    try:
        node = build(cfg, ctx, jobs, gates, hooks, registration, version)
        state = node.start()
    except Exception as error:  # noqa: BLE001
        log.error("robot node failed to start: %s", error, exc_info=True)
        if node is not None:
            # A partial start still left a loop thread, and possibly a peer and
            # a credential holding authority. Unwind it rather than leaking it
            # for the life of the process.
            try:
                node.stop()
            except Exception:  # noqa: BLE001
                pass
        return None
    log.info("robot node running as peer %s with %d capability(ies)",
             state.get("peer_id"), len(state.get("capabilities") or ()))
    return node


def status(cfg, node, registration=None):
    """What /api/status reports about the node."""
    if node is not None:
        return node.state.snapshot()
    return {"phase": "not-started", "peer_id": None, "domain_id": None,
            "capabilities": [], "resources": [],
            "error": "; ".join(blockers(cfg, registration)) or None}
