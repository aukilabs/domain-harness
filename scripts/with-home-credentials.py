#!/usr/bin/env python3
"""Load the user-supplied macOS RTF credentials only into a child environment.

Usage: python3 scripts/with-home-credentials.py COMMAND [ARG ...]
No secrets are printed, passed as command arguments, or written as plaintext.
"""
import os
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parent.parent
credential_file = Path(os.environ.get("SPATIAL_CREDENTIAL_FILE", root / "data/homeserver.rtf"))
if len(sys.argv) < 2:
    sys.exit("Usage: with-home-credentials.py COMMAND [ARG ...]")
text = subprocess.check_output(
    ["textutil", "-convert", "txt", "-stdout", str(credential_file)], text=True
)
fields = {}
for line in text.splitlines():
    parts = line.strip().split(None, 1)
    if len(parts) == 2:
        fields[parts[0].lower().rstrip(":=")] = parts[1].strip()
if not fields.get("key") or not fields.get("secret"):
    sys.exit("Credential file must contain Key and Secret lines (values not displayed).")
environment = os.environ.copy()
environment["AUKI_ACCESS_KEY"] = fields["key"]
environment["AUKI_APP_SECRET"] = fields["secret"]
os.execvpe(sys.argv[1], sys.argv[1:], environment)
