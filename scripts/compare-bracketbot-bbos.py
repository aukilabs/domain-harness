#!/usr/bin/env python3
"""Read-only BBOS samples around a user-approved 60-second runner stop.

The supervisor is restored in finally with its exact argv and in-memory env.
No Home, BBOS, profile, credential, or map files are changed.
"""
import shlex
import subprocess

PROBE = r'''
from bbos import Reader
import json,time,sys,statistics
readers={n:Reader(n,keeptime=False) for n in ['slam.health','slam.pose']}
for r in readers.values(): r.__enter__()
latest={}; samples=[]; phase=sys.argv[1]; duration=int(sys.argv[2])
def cpu():
    with open('/proc/stat') as f: v=list(map(int,f.readline().split()[1:9]))
    return sum(v),v[3]+v[4]
previous=cpu(); start=time.monotonic(); next_sample=start+1
while time.monotonic()-start<duration:
    for name,r in readers.items():
        if r.ready(): latest[name]=r.data.copy()
    if time.monotonic()>=next_sample:
        next_sample+=1; now=time.time_ns(); current=cpu(); delta=current[0]-previous[0]
        busy=100*(1-(current[1]-previous[1])/delta) if delta else 0; previous=current
        h=latest.get('slam.health'); p=latest.get('slam.pose')
        sample=dict(cpu_busy=round(busy,1),pose_age_ms=None if p is None else round((now-int(p['timestamp'].astype('int64')))/1e6),
            health_age_ms=None if h is None else round((now-int(h['timestamp'].astype('int64')))/1e6),
            lost=None if h is None else bool(h['vo_lost']),stalled=None if h is None else bool(h['stalled']),
            localized=None if h is None else bool(h['localized']),gap_ms=None if h is None else float(h['last_gap_ms']),
            pgo=None if p is None else int(p['pgo_count']))
        samples.append(sample)
        if len(samples)%10==0: print('DIAG '+json.dumps(dict(phase=phase,seconds=len(samples),latest=sample)),flush=True)
    time.sleep(.03)
def stats(key):
    values=sorted(s[key] for s in samples if s[key] is not None)
    return None if not values else dict(median=round(statistics.median(values),1),p95=round(values[min(len(values)-1,int(len(values)*.95))],1),maximum=round(max(values),1))
print('DIAG '+json.dumps(dict(phase=phase,elapsed_s=round(time.monotonic()-start,2),samples=len(samples),
    cpu_busy=stats('cpu_busy'),pose_age_ms=stats('pose_age_ms'),health_age_ms=stats('health_age_ms'),gap_ms=stats('gap_ms'),
    lost_samples=sum(s['lost'] is True for s in samples),stalled_samples=sum(s['stalled'] is True for s in samples),
    unlocalized_samples=sum(s['localized'] is False for s in samples))),flush=True)
'''

REMOTE = r'''
import json,os,signal,subprocess,time
from pathlib import Path
state=Path('/home/bracketbot/deployments/domain-harness/runner/state')
pid=int((state/'runner.pid').read_text()); process=Path(f'/proc/{pid}')
argv=[x.decode() for x in (process/'cmdline').read_bytes().split(b'\0') if x]
expected='/home/bracketbot/deployments/domain-harness/runner/source/scripts/supervise-bracketbot.py'
assert len(argv)>1 and argv[1]==expected and argv[argv.index('--state')+1]==str(state), 'Unexpected supervisor'
environment=dict(x.decode().split('=',1) for x in (process/'environ').read_bytes().split(b'\0') if x)
cwd=os.readlink(process/'cwd')
wrapper='/home/bracketbot/deployments/domain-harness/runner/source/scripts/bracketbot-python.py'
def probe(phase,seconds):
    subprocess.run(['/usr/bin/python3',wrapper,'-c',PROBE,phase,str(seconds)],check=True,timeout=seconds+20)
def alive():
    try: return process.exists() and (process/'stat').read_text().split(') ',1)[1].split()[0]!='Z'
    except FileNotFoundError: return False
probe('runner_on',20)
assert int((state/'runner.pid').read_text())==pid,'Supervisor changed during baseline'
stopped=False
try:
    os.kill(pid,signal.SIGTERM);stopped=True
    deadline=time.monotonic()+35
    while alive():
        if time.monotonic()>deadline: raise RuntimeError('Supervisor did not stop cleanly')
        time.sleep(.25)
    # Confirm no remaining Rust runner before measuring the off phase.
    pids=subprocess.check_output(['pgrep','-x','bracketbot-runn'],text=True) if subprocess.run(['pgrep','-x','bracketbot-runn'],stdout=subprocess.DEVNULL).returncode==0 else ''
    assert not pids.strip(),'A runner is still running; do not call this an off comparison'
    print('DIAG '+json.dumps(dict(event='runner_stopped',bbos_untouched=True)),flush=True)
    probe('runner_off',60)
finally:
    if stopped and not alive():
        with (state/'supervisor.log').open('ab',buffering=0) as log:
            child=subprocess.Popen(argv,cwd=cwd,env=environment,stdin=subprocess.DEVNULL,stdout=log,stderr=log,start_new_session=True)
        print('DIAG '+json.dumps(dict(event='runner_restored',supervisor_pid=child.pid,preserved_arguments=argv[2:])),flush=True)
    elif stopped:
        print('DIAG '+json.dumps(dict(event='stop_incomplete',no_duplicate_started=True)),flush=True)
del environment
time.sleep(4)
probe('runner_restored',20)
p=state/'robot-control.json';d=json.loads(p.read_text());m=d.get('registries',{}).get('local_mapping',{})
print('DIAG '+json.dumps(dict(event='runner_status',age_s=round(time.time()-p.stat().st_mtime,2),status=m.get('status'),scans=m.get('scan_progress',{}).get('integrated_observations'))),flush=True)
'''

if __name__ == '__main__':
    source='PROBE='+repr(PROBE)+'\n'+REMOTE
    subprocess.run(['bb','ssh','0184',shlex.join(['/usr/bin/python3','-c',source])],check=True,timeout=180)
