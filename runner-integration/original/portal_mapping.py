"""Opt-in, read-only teleop mapping. No actuation or legacy-map writes.

The native SDK extension owns Camera/Detector/Map Components and serves their
Products on the existing robot peer. HTTP only mirrors its retained map snapshot.
"""
import copy
import json
import math
import os
from pathlib import Path
import re
import threading
import uuid

import numpy as np
from scipy.spatial.transform import Rotation

from utils.transforms import convert_pose_to_transform


def rigid(value):
    m = np.asarray(value, dtype=float)
    if (m.shape != (4, 4) or not np.isfinite(m).all()
            or not np.allclose(m[3], [0, 0, 0, 1], atol=1e-8)
            or not np.allclose(m[:3, :3].T @ m[:3, :3], np.eye(3), atol=1e-8)
            or not np.isclose(np.linalg.det(m[:3, :3]), 1, atol=1e-8)):
        raise ValueError("expected a finite rigid transform")
    return m


class GalbotMappingSource:
    """Sensor adapter only: raw RGB, intrinsics and timestamped transforms."""
    def __init__(self, ctx, options):
        from utils.sdk import galbot_sensor_types, decode_compressed_RgbData
        self.ctx, self.options = ctx, options
        self.sensor_type = galbot_sensor_types[options["camera_key"]]
        self.decode = decode_compressed_RgbData
        self.last_timestamp = None

    def read(self):
        import cv2
        o = self.options
        obs = self.ctx.galbot.robot.get_synced_observation([self.sensor_type], True)
        frame = obs.rgb_data_map.get(self.sensor_type) if obs else None
        if frame is None:
            return None
        timestamp = int(frame.header.timestamp_ns)
        if timestamp < 0:
            raise ValueError("invalid sensor timestamp")
        if self.last_timestamp is not None:
            if timestamp < self.last_timestamp:
                raise ValueError("sensor clock regressed; start a new clock/session")
            if timestamp == self.last_timestamp:
                return None
        robot = self.ctx.galbot.robot
        base, base_ts = robot.get_transform(o["odom_frame"], "base_link", timestamp)
        cam, cam_ts = robot.get_transform("base_link", o["camera_link_frame"], timestamp)
        if int(base_ts) != timestamp or int(cam_ts) != timestamp:
            raise ValueError("capture-time TF unavailable; refusing latest-pose fallback")
        camera_to_odom = (convert_pose_to_transform(base) @ convert_pose_to_transform(cam)
                          @ rigid(o["optical_to_camera_link"]))
        image = self.decode(frame)
        intrinsics = self.ctx.galbot.get_cam_intrinsics(o["camera_key"]).copy()
        if o["distortion"]:
            image = cv2.undistort(image, intrinsics, np.asarray(o["distortion"]), None, intrinsics)
        self.last_timestamp = timestamp
        return timestamp, rigid(camera_to_odom), cv2.cvtColor(image, cv2.COLOR_BGR2RGB), intrinsics


def resolve_portal_metadata(ctx, domain, portal):
    """Read canonical Portal metadata from DDS using the existing robot grant.

    Verified against DDS HandleGetLighthouseInDomain and robot domain-token's
    DDS audience. DS pose records are not the canonical metadata response.
    """
    from urllib.parse import quote
    client = ctx.domain_client
    if client.assigned_domain_id != domain:
        raise ValueError("Portal lookup domain mismatch")
    token, _server, resolved = client.domain_session()
    if resolved != domain:
        raise ValueError("Portal grant domain mismatch")
    response = client._request(
        "GET", client.dds_base_url.rstrip("/") + "/api/v1/domains/" + quote(domain, safe="")
        + "/lighthouses/" + quote(portal, safe=""),
        headers={"Authorization": "Bearer " + token}, timeout=3)
    record = response.json()
    if record.get("domain_id") != domain:
        raise ValueError("Portal response domain mismatch")
    # Rust validates UUID/shortcode matching, timestamps and physical size.
    return json.dumps(record, allow_nan=False)


def make_pipeline(ctx, options, session_id, image, intrinsics):
    import auki_sdk
    peer = getattr(ctx, "mapping_peer", None)
    clock = getattr(ctx, "mapping_publication_clock", None)
    if peer is None or clock is None:
        raise ValueError("mapping requires the existing robot peer and its registered publication clock")
    if peer.domain_id != options["domain_id"]:
        raise ValueError("peer and mapping domain disagree")
    import auki_registry as registry
    optical = registry.frame_ros_optical(peer.peer_id, options["optical_frame"])
    optical.pop("hash", None)
    config = dict(optical_registry=optical, domain_id=options["domain_id"], map_id="galbot-" + session_id,
                  map_frame=options["legacy_frame"] + "-zup", optical_frame=options["optical_frame"],
                  capture_clock=options["capture_clock"], publication_clock=clock,
                  width=int(image.shape[1]), height=int(image.shape[0]),
                  fx=float(intrinsics[0,0]), fy=float(intrinsics[1,1]),
                  cx=float(intrinsics[0,2]), cy=float(intrinsics[1,2]))
    return auki_sdk.PortalMappingPipeline.mount(peer, json.dumps(config),
                                               lambda domain, portal: resolve_portal_metadata(ctx, domain, portal))


def validate_options(o):
    for key in ("domain_id", "legacy_frame", "odom_frame", "odom_session_id",
                "camera_key", "camera_link_frame", "optical_frame"):
        if not isinstance(o.get(key), str) or not o[key].strip() or "REPLACE_WITH" in o[key]:
            raise ValueError("missing explicit " + key)
    rigid(o["optical_to_camera_link"])
    clock = o.get("capture_clock", {})
    if any(not isinstance(clock.get(k), str) or not clock[k] for k in ("peer_id", "id", "hash")):
        raise ValueError("capture_clock requires peer_id, id and definition hash")
    if not re.fullmatch(r"[0-9a-f]{32}", clock["hash"]):
        raise ValueError("capture_clock hash must be a 32-character registry definition hash")
    if not isinstance(o.get("distortion"), list) or not np.isfinite(o["distortion"]).all():
        raise ValueError("explicit camera distortion coefficients required (empty for rectified)")
    fps = float(o.get("fps", 1))
    if not math.isfinite(fps) or not 0 < fps <= 5:
        raise ValueError("fps must be in (0, 5]")
    return fps


class PortalMappingSession:
    def __init__(self, ctx, options, root, source_factory=GalbotMappingSource, pipeline_factory=make_pipeline):
        self.ctx, self.options = ctx, copy.deepcopy(options)
        self.fps = validate_options(options)
        self.root = Path(root)
        self.source_factory, self.pipeline_factory = source_factory, pipeline_factory
        self.lock = threading.RLock()
        self.stop_event = threading.Event()
        self.thread = None
        self.pipeline = None
        self.result = None
        self.state = {"status": "idle", "portal_count": 0, "error": None}

    def status(self):
        with self.lock:
            return copy.deepcopy(self.state)

    def start(self):
        with self.lock:
            if self.thread is not None and self.thread.is_alive():
                raise ValueError("mapping is already active")
            ok, domain, calibration, _ = self.ctx.calibration_state.snapshot()
            if not ok or domain != self.options["domain_id"]:
                raise ValueError("calibrate into the configured domain first")
            if not self.ctx.robot_state.snapshot()["ready"]:
                raise ValueError("robot is not ready")
            source = self.source_factory(self.ctx, self.options)
            first = source.read()
            if first is None:
                raise ValueError("no capture-time camera observation available")
            _, _, image, intrinsics = first
            if self.pipeline is not None:
                self.pipeline.close()
                self.pipeline = None
            session_id = uuid.uuid4().hex
            self.calibration = rigid(calibration).copy()
            self.pipeline = self.pipeline_factory(self.ctx, self.options, session_id, image, intrinsics)
            try:
                self.directory = self.root / session_id
                self.directory.mkdir(parents=True)
                self.state = {"status": "mapping", "session_id": session_id, "portal_count": 0, "error": None}
                self.intrinsics = intrinsics.copy()
                self.shape = image.shape
                self._save(json.loads(self.pipeline.snapshot()), None)
            except Exception as exc:
                self.pipeline.close()
                self.pipeline = None
                self.state = {"status": "failed", "portal_count": 0, "error": str(exc)}
                raise
            self.stop_event.clear()
            self.thread = threading.Thread(target=self._run, args=(source, first), daemon=True, name="portal-mapping")
            self.thread.start()
            return self.status()

    def _save(self, result, capture):
        bundle = {"schema": "galbot.portal-mapping/v2", "capture": capture, "configuration": self.options,
                  "pinned_calibration": {"from_frame_id": self.options["odom_frame"],
                                         "to_frame_id": self.options["legacy_frame"],
                                         "matrix": self.calibration.tolist()}, **result}
        temp = self.directory / "map.json.tmp"
        temp.write_text(json.dumps(bundle, allow_nan=False))
        temp.replace(self.directory / "map.json")
        temp = self.directory / "map.usda.tmp"
        temp.write_text(result["inspection_usda"])
        temp.replace(self.directory / "map.usda")
        self.result = bundle

    def _run(self, source, first):
        try:
            sample = first
            # Explicit convention conversion: OpenGL (x,y,z) -> (x,-z,y).
            basis = np.array([[1,0,0,0], [0,0,-1,0], [0,1,0,0], [0,0,0,1]], dtype=float)
            while not self.stop_event.is_set():
                ok, domain, _, _ = self.ctx.calibration_state.snapshot()
                if not ok or domain != self.options["domain_id"]:
                    raise ValueError("domain/calibration lost; start a new session")
                if sample is not None:
                    timestamp, camera_to_odom, image, intrinsics = sample
                    if image.shape != self.shape or not np.array_equal(intrinsics, self.intrinsics):
                        raise ValueError("camera calibration changed; start a new session")
                    pose = rigid(basis @ self.calibration @ rigid(camera_to_odom))
                    qx,qy,qz,qw = Rotation.from_matrix(pose[:3,:3]).as_quat()
                    labelled = dict(from_frame_id=self.options["optical_frame"],
                                    to_frame_id=self.options["legacy_frame"] + "-zup",
                                    translation=pose[:3,3].tolist(), rotation_wxyz=[qw,qx,qy,qz])
                    result = json.loads(self.pipeline.process(image.tobytes(), timestamp, json.dumps(labelled)))
                    with self.lock:
                        self._save(result, {"timestamp_ns": timestamp, "clock": self.options["capture_clock"],
                                            "camera_to_map": labelled})
                        self.state["portal_count"] = len(result["snapshot"]["scenegraph"]["anchors"])
                        self.state["snapshot_reference"] = result["snapshot_reference"]
                        self.state["unresolved_portals"] = result.get("unresolved_portals", [])
                if self.stop_event.wait(1 / self.fps):
                    break
                sample = source.read()
        except Exception as exc:
            with self.lock:
                self.state.update(status="failed", error=str(exc))
        finally:
            with self.lock:
                if self.state["status"] != "failed":
                    self.state["status"] = "stopped"

    def stop(self, timeout=1):
        with self.lock:
            self.stop_event.set()
            thread = self.thread
        if thread is not None:
            thread.join(timeout)
        with self.lock:
            if thread is not None and thread.is_alive():
                self.state["status"] = "stopping"
        return self.status()

    def close(self):
        self.stop(timeout=6)
        if self.pipeline is not None:
            self.pipeline.close()
            self.pipeline = None

    def snapshot(self):
        with self.lock:
            return copy.deepcopy(self.result)


def mapping_node_class(base, ctx):
    """Mount on the SAME peer; unmount before the kit closes its authority."""
    import asyncio
    class MappingEndpoint:
        async def close(self):
            ctx.mapping_peer = None
            session = getattr(ctx, "portal_mapping_session", None)
            if session is not None:
                await asyncio.get_running_loop().run_in_executor(None, session.close)

    class MappingNode(base):
        def _mount(self, sdk):
            super()._mount(sdk)
            if not hasattr(sdk, "PortalMappingPipeline"):
                raise RuntimeError("install the Component-enabled auki_sdk extension for mapping")
            ctx.mapping_peer = self._peer
            ctx.mapping_publication_clock = dict(self._written.clock)
            self._endpoints.append(MappingEndpoint())
    return MappingNode


def register_mapping(app, ctx):
    """Disabled unless a host-owned mapping configuration is supplied."""
    from flask import jsonify, Response
    path = os.environ.get("GALBOT_PORTAL_MAPPING_CONFIG")
    if not path:
        return None
    options = json.loads(Path(path).read_text())
    session = PortalMappingSession(ctx, options, Path(ctx.cfg.capture_root) / "portal-maps")
    ctx.portal_mapping_session = session

    @app.post("/api/portal_mapping/start")
    def start():
        try:
            return jsonify(session.start())
        except (ValueError, KeyError, ImportError, OSError, RuntimeError) as exc:
            return jsonify(error=str(exc)), 409

    @app.post("/api/portal_mapping/stop")
    def stop():
        return jsonify(session.stop())

    @app.get("/api/portal_mapping/status")
    def status():
        return jsonify(session.status())

    @app.get("/api/portal_mapping/snapshot")
    def snapshot():
        value = session.snapshot()
        return (jsonify(value), 200) if value is not None else (jsonify(error="no mapping session"), 404)

    @app.get("/api/portal_mapping/map.usda")
    def usda():
        value = session.snapshot()
        if value is None:
            return jsonify(error="no mapping session"), 404
        return Response(value["inspection_usda"], mimetype="text/plain")

    return session
