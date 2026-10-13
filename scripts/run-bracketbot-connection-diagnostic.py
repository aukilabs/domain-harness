#!/usr/bin/env python3
"""Deploy the minimal peer + sole LED controller. Never start/change BBOS.

Supply AUKI_ACCESS_KEY/AUKI_APP_SECRET in the environment (or invoke through
with-home-credentials.py). Credentials go over SSH stdin, never into files.
The binary must already be cross-built for ARM64 Linux on the laptop.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import shlex
import subprocess
import tarfile
import tempfile

ROOT = '/home/bracketbot/diagnostics/connection-peer'
START = r'''
import json,os,signal,socket,subprocess,sys,time,shutil,hashlib
from pathlib import Path
root=Path('/home/bracketbot/diagnostics/connection-peer')
request=json.load(sys.stdin)
if socket.gethostname() != 'bracketbot-'+request['robot']:
    raise SystemExit('Robot hostname mismatch; refusing deployment')
staged=root/'bin/auki-connection-diagnostic.next'
if not staged.is_file() or hashlib.sha256(staged.read_bytes()).hexdigest()!=request['sha256']:
    raise SystemExit('Staged binary is missing or differs from validated build')
def commands():
    for proc in Path('/proc').iterdir():
        if not proc.name.isdigit(): continue
        try: args=(proc/'cmdline').read_bytes().split(b'\0')
        except (OSError,ProcessLookupError): continue
        yield int(proc.name),[a.decode(errors='replace') for a in args if a]
for pid,args in commands():
    if any(Path(a).name in ('bracketbot-runner','supervise-bracketbot.py','bracketbot_capture.py') for a in args[:3]):
        raise SystemExit('Full Auki runner/capture is active; refusing mixed diagnostic')
for file in ('bin/auki-connection-diagnostic.next','scripts/bracketbot-python.py','scripts/bracketbot_slam_led_diagnostic.py','peer.json'):
    if not (root/file).is_file(): raise SystemExit('Missing diagnostic file: '+file)
home=request['home']
if home.get('schema')!='spatial-harness.home-connection/v1' or not home.get('routes'):
    raise SystemExit('Invalid Home profile')
config=json.loads((root/'peer.json').read_text())
if config['domain_id']!=home['domain_id']: raise SystemExit('Domain mismatch')
env=os.environ.copy(); env.update(request['credentials'])
if request.get('poses'):
    address=home['routes'][0].split('/')
    if address[1]!='ip4': raise SystemExit('Diagnostic requires IPv4 Home route')
    with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as route:
        route.connect((address[2],int(address[4])))
        env['DIAGNOSTIC_LAN_IP']=route.getsockname()[0]
# Replace only this diagnostic or its original LED-only version, never BBOS.
for pid,args in commands():
    if any(Path(a).name=='bracketbot_slam_led_diagnostic.py' for a in args[:3]):
        os.kill(pid,signal.SIGTERM)
        for _ in range(120):
            if not Path('/proc',str(pid)).exists(): break
            time.sleep(.1)
        else: raise SystemExit('Previous diagnostic did not stop; leaving it alone')
binary=root/'bin/auki-connection-diagnostic'
if binary.exists():
    shutil.copy2(binary,binary.with_name(binary.name+'.previous-'+str(time.time_ns())))
(root/'bin/auki-connection-diagnostic.next').replace(binary)
(root/'home-connection.json').write_text(json.dumps(home)+'\n')
command=['python3',str(root/'scripts/bracketbot-python.py'),str(root/'scripts/bracketbot_slam_led_diagnostic.py')]
if request.get('poses'): command+=['--publish-poses']
command+=['--peer-command',str(root/'bin/auki-connection-diagnostic'),str(root/'peer.json'),
    str(root/'home-connection.json'),str(root/'diagnostic.identity')]
if request.get('portals'): command+=['--portals']
elif request.get('poses'): command+=['--poses']
with (root/'diagnostic.log').open('ab',buffering=0) as log:
    child=subprocess.Popen(command,cwd=root,env=env,stdin=subprocess.DEVNULL,stdout=log,stderr=log,start_new_session=True)
(root/'diagnostic.pid').write_text(str(child.pid)+'\n')
time.sleep(2)
if child.poll() is not None: raise SystemExit('Diagnostic exited; check diagnostic.log')
print(json.dumps({'controller_pid':child.pid,'directory':str(root),'bbos_changed':False,'autostart':False}))
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--robot', default='182')
    parser.add_argument('--home-connection', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--poses', action='store_true', help='Expose only the typed BBOS pose Product')
    parser.add_argument('--portals', action='store_true', help='Add one head camera and Arshak QR detector; implies poses')
    parser.add_argument('--start-staged', action='store_true', help='Resume after a completed transfer; verifies the staged binary hash before stopping anything')
    args = parser.parse_args()
    args.poses = args.poses or args.portals
    repo = Path(__file__).resolve().parents[1]
    credentials = {key: os.environ[key] for key in ('AUKI_ACCESS_KEY', 'AUKI_APP_SECRET')}
    home = json.loads(args.home_connection.read_text())
    # Check ELF architecture before copying anything to the robot.
    header = args.binary.read_bytes()[:20]
    if header[:4] != b'\x7fELF' or int.from_bytes(header[18:20], 'little') != 183:
        parser.error('binary must be ARM64 Linux ELF')
    ssh = ['bb', '--llm', 'ssh', args.robot]
    with tempfile.TemporaryFile() as payload:
        with tarfile.open(fileobj=payload, mode='w:gz', compresslevel=3) as archive:
            archive.add(args.binary, arcname='bin/auki-connection-diagnostic.next')
            # Only shared auth endpoints/Domain; never copy 182's routes or identity.
            source = json.loads((repo/'config/bracketbot-182.peer.json').read_text())
            config = json.dumps({key: source[key] for key in
                                 ('domain_id', 'api_url', 'dds_url', 'dms_url', 'login')}).encode()
            entry = tarfile.TarInfo('peer.json')
            entry.size = len(config)
            archive.addfile(entry, io.BytesIO(config))
            for name in ('bracketbot-python.py', 'bracketbot_slam_led_diagnostic.py', 'diagnostic-camera.py', 'bracketbot_capture.py'):
                archive.add(repo/'scripts'/name, arcname='scripts/'+name)
        payload.seek(0)
        if not args.start_staged:
            subprocess.run(ssh+[f'mkdir -p {ROOT} && tar -xzf - -C {ROOT}'], stdin=payload, check=True)
    subprocess.run(ssh+[shlex.join(['python3', '-c', START])],
                   input=json.dumps({'home':home,'credentials':credentials,'poses':args.poses,'portals':args.portals,'robot':args.robot,'sha256':hashlib.sha256(args.binary.read_bytes()).hexdigest()}).encode(), check=True)


if __name__ == '__main__':
    main()
