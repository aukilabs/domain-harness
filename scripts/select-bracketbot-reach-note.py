#!/usr/bin/env python3
"""Select a persistent Home note target on the running robot. No rebuild/restart."""
import argparse
import json
import math
import shlex
import subprocess
import time
import urllib.request
import urllib.parse
import uuid


def request_for(view,name):
    matches=[n for n in view['notes'] if n.get('text','').strip().casefold()==name.strip().casefold()
             and not n.get('phone_pose') and not n.get('anchor_robot')]
    if len(matches)!=1:raise ValueError(f'Expected one note named {name!r}; found {len(matches)}')
    note=matches[0];p=note['position']
    if len(p)!=3 or not all(isinstance(x,(float,int)) and not isinstance(x,bool) and math.isfinite(x) for x in p):
        raise ValueError('Note has no finite scene position')
    if note.get('anchor_surface',{}).get('frame',view['frame'])!=view['frame']:
        raise ValueError('Note belongs to another frame')
    return dict(schema='spatial.robot-reach-note/v1',request_id=str(uuid.uuid4()),
                requested_ns=time.time_ns(),requested_name=name,authority_peer_id=view['authority_peer_id'],
                frame=view['frame'],note={k:note[k] for k in ('id','text','position')})


REMOTE=r'''
import json,math,os,sys
from pathlib import Path
state=Path('/home/bracketbot/deployments/domain-harness/runner/state')
request=json.load(sys.stdin);home=json.loads((state/'home-connection.json').read_text())
assert request['schema']=='spatial.robot-reach-note/v1'
assert (request['authority_peer_id'],request['frame'])==(home['peer_id'],home['frame']), 'Home identity/frame mismatch'
assert request['note']['text'].strip().casefold()==request['requested_name'].strip().casefold()
assert len(request['note']['position'])==3 and all(math.isfinite(x) for x in request['note']['position'])
assert request['request_id'] and request['note']['id']
path=state/'reach-target.json';tmp=state/'reach-target.selection.tmp'
tmp.write_text(json.dumps(request,allow_nan=False));os.chmod(tmp,0o600);tmp.replace(path)
print(json.dumps({'selected_note':request['note'],'request_id':request['request_id'],'frame':request['frame'],
                  'runtime_target':str(path),'restart_required':False}))
'''


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('name');ap.add_argument('--home-url',default='http://127.0.0.1:4329')
    ap.add_argument('--scene',default='humanoid-hub-conference');ap.add_argument('--robot',default='0184')
    ap.add_argument('--preview',action='store_true',help='Resolve without sending a target')
    args=ap.parse_args()
    url=args.home_url.rstrip('/')+'/api/local-scenes/'+urllib.parse.quote(args.scene,safe='')
    with urllib.request.urlopen(url,timeout=10) as response:view=json.load(response)
    request=request_for(view,args.name)
    if args.preview:print(json.dumps(request,indent=2));return
    subprocess.run(['bb','ssh',args.robot,shlex.join(['/usr/bin/python3','-c',REMOTE])],
                   input=json.dumps(request,allow_nan=False),text=True,check=True)


if __name__=='__main__':main()
