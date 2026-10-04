#!/usr/bin/env python3
"""Run the single Galbot sensor and scenegraph runner. Use through with-home-credentials.py.
Credentials pass only through encrypted SSH stdin into the process environment.
"""
import argparse
import ipaddress
import json
import os
import shlex
import subprocess
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--mapping-camera',choices=['left_arm','right_arm'],help='Wrist camera for portal localization and RGB-D')
parser.add_argument('--host', default='192.168.9.95', help='Galbot IPv4 address')
parser.add_argument('--root', help='Absolute runner installation directory')
parser.add_argument('--binary', help='Absolute runner executable path')
options = parser.parse_args()
host = str(ipaddress.IPv4Address(options.host))
root = options.root or ('/userdata/sam_ws/galbot-g1-runner-portal-mapping/single_runner'
                        if host == '192.168.9.95' else '/home/galbot/auki/spatial-harness-runner')
binary = options.binary or root + '/bin/galbot-runner'
if not root.startswith('/') or not binary.startswith('/'):
    parser.error('root and binary must be absolute paths')
args = [binary, root + '/state/peer.json',
        root + '/state/local-scene.json', root + '/state/connection.json']
values = {key: os.environ[key] for key in ("AUKI_ACCESS_KEY", "AUKI_APP_SECRET")}
values["SPATIAL_LOCAL_CAPTURE"] = root + "/scripts/capture-runner.py"
values["GALBOT_MAPPING_CAMERA"] = options.mapping_camera or ('left_arm' if host == '192.168.9.87' else 'right_arm')
values["SPATIAL_MAPPING_CAMERA_FRAME"] = values["GALBOT_MAPPING_CAMERA"] + '_camera_color_optical_frame'
code = "import json,os,sys; os.environ.update(json.load(sys.stdin)); os.chdir(sys.argv[1]); os.execvp(sys.argv[2],sys.argv[2:])"
command = "python3 -c " + shlex.quote(code) + " " + shlex.join([root, *args])
result = subprocess.run(["ssh", "-o", "StrictHostKeyChecking=yes", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=4", "galbot@" + host, command],
                        input=json.dumps(values), text=True)
sys.exit(result.returncode)
