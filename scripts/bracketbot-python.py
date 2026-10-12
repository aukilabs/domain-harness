#!/usr/bin/env python3
"""Use BBOS's existing Nix/Python environment without rebuilding or changing it."""
import json
import os
from pathlib import Path
import sys


def main():
    if len(sys.argv) < 2:
        raise SystemExit("Usage: bracketbot-python.py SCRIPT [ARGS...]")
    root = Path(os.environ.get("BBOS_ROOT", "/home/bracketbot/bbos"))
    args = sys.argv[1:]
    name = 'depth'
    if args[:1] == ['--daemon']:
        name = args[1]
        if name not in ('depth', 'arm_left', 'speaker'):
            raise SystemExit('Unsupported read-only adapter runtime')
        args = args[2:]
    daemon = root / 'bbos/daemons' / name
    with (daemon / ".devenv/bbos-env.json").open() as source:
        cached = json.load(source)
    env = os.environ.copy()
    for key in ("LD_LIBRARY_PATH", "PATH", "PYTHONPATH", "NIX_LD_LIBRARY_PATH"):
        if key in cached:
            env[key] = cached[key]
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    python = str(daemon / ".venv/bin/python")
    os.execve(python, [python, *args], env)


if __name__ == "__main__":
    main()
