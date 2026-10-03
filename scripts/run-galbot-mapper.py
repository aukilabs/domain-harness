#!/usr/bin/env python3
"""Compatibility entry point: launch the single Galbot runner, never a second mapper peer."""
from pathlib import Path
import runpy
runpy.run_path(str(Path(__file__).with_name('run-galbot-runner.py')), run_name='__main__')
