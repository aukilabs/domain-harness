#!/usr/bin/env python3
"""Restart the existing runner with identical arguments and environment.

Leaves Home, credentials files, profiles, robot identity and motion flags intact.
The existing supervisor stops its drive controller before restarting.
"""
import shlex
import subprocess

REMOTE = r'''
import json,os,signal,subprocess,time
from pathlib import Path
state=Path('/home/bracketbot/deployments/domain-harness/runner/state')
pid=int((state/'runner.pid').read_text())
process=Path(f'/proc/{pid}')
argv=[x.decode() for x in (process/'cmdline').read_bytes().split(b'\0') if x]
expected='/home/bracketbot/deployments/domain-harness/runner/source/scripts/supervise-bracketbot.py'
if len(argv)<2 or argv[1]!=expected or '--state' not in argv or argv[argv.index('--state')+1]!=str(state):
    raise SystemExit('Refusing to restart an unrelated process')
# Reuse authentication only in memory; never print or persist the environment.
environment=dict(x.decode().split('=',1) for x in (process/'environ').read_bytes().split(b'\0') if x)
cwd=os.readlink(process/'cwd')
if int((state/'runner.pid').read_text())!=pid: raise SystemExit('Supervisor changed; retry')
os.kill(pid,signal.SIGTERM)
deadline=time.monotonic()+100
while process.exists():
    try:
        status=(process/'stat').read_text().split(') ',1)[1].split()[0]
        if status=='Z': break
    except FileNotFoundError: break
    if time.monotonic()>deadline: raise SystemExit('Previous supervisor is still stopping; no second runner started')
    time.sleep(.25)
with (state/'supervisor.log').open('ab',buffering=0) as log:
    child=subprocess.Popen(argv,cwd=cwd,env=environment,stdin=subprocess.DEVNULL,
                           stdout=log,stderr=log,start_new_session=True)
del environment
print(json.dumps({'supervisor_pid':child.pid,'preserved_arguments':argv[2:]}),flush=True)
deadline=time.monotonic()+45
while time.monotonic()<deadline:
    if child.poll() is not None: raise SystemExit('Supervisor exited; inspect supervisor.log')
    path=state/'robot-presence-status.json'
    if path.exists() and time.time()-path.stat().st_mtime < 2:
        print(path.read_text(),flush=True); break
    time.sleep(.25)
else: print('Runner is starting; inspect robot-presence-status.json',flush=True)
'''


if __name__ == '__main__':
    subprocess.run(['bb','ssh','0184',shlex.join(['/usr/bin/python3','-c',REMOTE])],check=True)
