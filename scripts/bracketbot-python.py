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
    daemon = root / "bbos/daemons/depth"
    with (daemon / ".devenv/bbos-env.json").open() as source:
        cached = json.load(source)
    env = os.environ.copy()
    for key in ("LD_LIBRARY_PATH", "PATH", "PYTHONPATH", "NIX_LD_LIBRARY_PATH"):
        if key in cached:
            env[key] = cached[key]
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    python = str(daemon / ".venv/bin/python")
    os.execve(python, [python, *sys.argv[1:]], env)


if __name__ == "__main__":
    main()
