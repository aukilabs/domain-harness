#!/usr/bin/env python3
"""Run the isolated Galbot scene reader over SSH; never invoke robot controls.

Use via with-home-credentials.py. Credentials travel only over encrypted SSH
stdin into the consumer environment, not shell arguments or files on the robot.
Default: fetch once and shut down. --watch follows for at most ten minutes.
"""
import json
import os
import shlex
import subprocess
import sys

if sys.argv[1:] not in ([], ["--watch"]):
    sys.exit("Usage: run-galbot-reader.py [--watch]")
values = {key: os.environ[key] for key in ("AUKI_ACCESS_KEY", "AUKI_APP_SECRET")}
root = "/home/galbot/auki/spatial-harness-reader"
args = [root + "/target/debug/spatial-runner", "--config", root + "/state/reader.json",
        "--home", root + "/state/home-connection.json", "--output", root + "/state/scene.json"]
if "--watch" not in sys.argv:
    args.append("--once")
else:
    args = ["timeout", "--signal=INT", "600", *args]
code = "import json,os,sys; os.environ.update(json.load(sys.stdin)); os.execvp(sys.argv[1],sys.argv[1:])"
command = "python3 -c " + shlex.quote(code) + " " + shlex.join(args)
result = subprocess.run(["ssh", "-o", "StrictHostKeyChecking=yes", "galbot@192.168.9.95", command],
                        input=json.dumps(values), text=True)
sys.exit(result.returncode)
