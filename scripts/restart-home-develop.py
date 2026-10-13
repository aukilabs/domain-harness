#!/usr/bin/env python3
"""Run the working checkout's Home build with the primary checkout's saved data.

Stop the old Home in its Terminal first. Optional --reset-scene forgets only
that scene's portals and voxels through Home's same-origin, snapshot-checked API.
Credentials remain inside the existing credential helper's child environment.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reset-scene", choices=["humanoid-hub-conference", "nacs"])
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    primary = root
    git_file = root / ".git"
    if git_file.is_file():
        pointer = git_file.read_text().strip()
        if pointer.startswith("gitdir: "):
            primary = Path(pointer.removeprefix("gitdir: ")).parent.parent.parent
    environment = os.environ.copy()
    saved = primary / "data/bracketbot-verification"
    defaults = {
        "SPATIAL_BIND": "0.0.0.0",
        "SPATIAL_UI_HOSTS": "192.168.1.26:4329",
        "SPATIAL_PORT": "4329",
        "HOME_CHAT_MODEL": "qwen3.5:35b-a3b",
        "SPATIAL_DATA": str(saved / "home.json"),
        "SPATIAL_PEER_CONFIG": str(saved / "home-peer.json"),
        "SPATIAL_CREDENTIAL_FILE": str(primary / "data/homeserver.rtf"),
    }
    for key, value in defaults.items():
        environment.setdefault(key, value)
    # A dedicated deployed artifact, not Cargo's shared output path (which
    # another checkout/build can replace while this launcher is in use).
    binary = root / "target/home-origin/spatial-home"
    if not binary.is_file():
        parser.error("Deploy the verified Home build to target/home-origin/spatial-home first")
    # Fail rather than accidentally replacing an unreadable saved configuration.
    for key in ("SPATIAL_PEER_CONFIG", "SPATIAL_CREDENTIAL_FILE"):
        with open(environment[key], "rb"):
            pass
    port = int(environment["SPATIAL_PORT"])
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=1):
            parser.error(f"Port {port} is occupied. Stop the old Home with Ctrl+C first.")
    except (ConnectionRefusedError, TimeoutError):
        pass
    child = subprocess.Popen([sys.executable, str(root / "scripts/with-home-credentials.py"), str(binary)],
                             cwd=root, env=environment)
    try:
        if args.reset_scene:
            base = f"http://127.0.0.1:{port}"
            endpoint = f"{base}/api/local-scenes/{args.reset_scene}"
            deadline = time.monotonic() + 60
            while True:
                if child.poll() is not None:
                    raise RuntimeError("Home exited before reset; no reset was requested")
                try:
                    with urllib.request.urlopen(endpoint + "/snapshot", timeout=2) as response:
                        snapshot = json.load(response)
                    break
                except (urllib.error.URLError, TimeoutError):
                    if time.monotonic() >= deadline:
                        raise RuntimeError("Home did not become ready; no reset was requested")
                    time.sleep(0.5)
            while True:
                request = urllib.request.Request(endpoint + "/reset-mapping", data=json.dumps(snapshot).encode(),
                    headers={"Content-Type": "application/json", "Origin": base}, method="POST")
                try:
                    with urllib.request.urlopen(request, timeout=15) as response:
                        print("Reset receipt:", json.dumps(json.load(response)), flush=True)
                    break
                except urllib.error.HTTPError as error:
                    detail = error.read(4096).decode("utf-8", errors="replace")
                    # HTTP may start before the SDK scene writer is mounted.
                    # Retry only this explicit rejection, never an ambiguous
                    # timeout after a potentially successful destructive write.
                    if error.code == 422 and '"Unknown local scene"' in detail and time.monotonic() < deadline:
                        if child.poll() is not None:
                            raise RuntimeError("Home exited before its scene writer was ready") from error
                        time.sleep(0.5)
                        with urllib.request.urlopen(endpoint + "/snapshot", timeout=2) as response:
                            snapshot = json.load(response)
                        continue
                    if error.code == 404:
                        raise RuntimeError(f"Home binary lacks the reset endpoint: {binary}. Rebuild and deploy it; no reset was performed.") from error
                    raise RuntimeError(f"Home rejected the reset (HTTP {error.code}): {detail}") from error
        print(f"Home: http://127.0.0.1:{port}/ — keep this Terminal open.", flush=True)
        return child.wait()
    except KeyboardInterrupt:
        child.terminate()
        return child.wait()
    except Exception:
        child.terminate()
        child.wait()
        raise


if __name__ == "__main__":
    sys.exit(main())
