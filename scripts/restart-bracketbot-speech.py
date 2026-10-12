#!/usr/bin/env python3
"""Deploy/restart only volume speech; no mapping, BBOS or motion restart."""
import argparse
from pathlib import Path
import shlex
import subprocess
import tarfile
import tempfile

REMOTE=r'''
import os,signal,subprocess,time,json
from pathlib import Path
state=Path('/home/bracketbot/deployments/domain-harness/runner/state')
script=state.parent/'source/scripts/speak-bracketbot-volumes.py'
matches=[]
for process in Path('/proc').iterdir():
    if not process.name.isdigit():continue
    try:argv=[x.decode() for x in (process/'cmdline').read_bytes().split(b'\0') if x]
    except (OSError,UnicodeError):continue
    if len(argv)>1 and argv[1]==str(script) and '--state' in argv and argv[argv.index('--state')+1]==str(state):matches.append(int(process.name))
assert len(matches)<=1,'Multiple speech services; inspect ownership'
for pid in matches:
    os.kill(pid,signal.SIGTERM)
    deadline=time.monotonic()+6
    while Path(f'/proc/{pid}').exists():
        try:
            if Path(f'/proc/{pid}/stat').read_text().split(') ',1)[1].split()[0]=='Z':break
        except FileNotFoundError:break
        if time.monotonic()>deadline:raise SystemExit('Speech service did not stop cleanly')
        time.sleep(.1)
with (state/'volume-speech.log').open('ab',buffering=0) as log:
    child=subprocess.Popen(['/usr/bin/python3',str(script),'--state',str(state)],stdin=subprocess.DEVNULL,stdout=log,stderr=log,start_new_session=True)
print(json.dumps(dict(speech_pid=child.pid,runner_untouched=True)),flush=True)
'''

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--robot',required=True);args=parser.parse_args()
    repo=Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory() as directory:
        archive=Path(directory)/'speech.tar'
        with tarfile.open(archive,'w') as tar:tar.add(repo/'scripts/speak-bracketbot-volumes.py',arcname='speak-bracketbot-volumes.py')
        with archive.open('rb') as payload:
            subprocess.run(['bb','--llm','ssh',args.robot,'tar -xf - -C /home/bracketbot/deployments/domain-harness/runner/source/scripts'],stdin=payload,check=True,timeout=30)
        subprocess.run(['bb','--llm','ssh',args.robot,shlex.join(['/usr/bin/python3','-c',REMOTE])],check=True,timeout=30)
