#!/usr/bin/env python3
"""Add pose publishing to the deployed runner, preserving all other sources.

Does not rebuild Home or change robot configuration, scripts, or motion flags.
Starting/restarting remains a separate operation.
"""
import json
from pathlib import Path
import shlex
import subprocess

REMOTE = r"""
import hashlib,json,os,subprocess,sys
from pathlib import Path
request=json.load(sys.stdin)
root=Path('/home/bracketbot/deployments/domain-harness/runner'); source=root/'source'
files={name:(source/name).read_text() for name in ('src/lib.rs','src/mobile_pose.rs','src/robot_runner.rs')}
if 'pub mod robot_presence;' not in files['src/lib.rs']:
    files['src/lib.rs'] += '\npub mod robot_presence;\n'
if 'pub async fn send_component<' not in files['src/mobile_pose.rs']:
    marker='    use auki_component_protocol::{CatalogResponse, ComponentProtocolClient};'
    extra='''    send_component(peer, home, COMPONENT, update).await
}
pub async fn send_component<T: auki_components::ContractType + Serialize>(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
    component: &str,
    update: &T,
) -> Result<()> {
'''
    text=files['src/mobile_pose.rs']; start=text.index('pub async fn send(')
    at=text.index(marker,start); text=text[:at]+extra+text[at:]
    old='home.scoped_component(COMPONENT)'
    before,after=text.split('pub async fn send_component<',1)
    if after.count(old)!=1: raise SystemExit('Unexpected presence sender: refuse to patch')
    files['src/mobile_pose.rs']=before+'pub async fn send_component<'+after.replace(old,'home.scoped_component(component)',1)
text=files['src/robot_runner.rs']
if '    let pose_work = async {' not in text:
    marker='    let health = async {'
    if text.count(marker)!=1 or text.count('_=drive_work=>{}')!=1:
        raise SystemExit('Unexpected runner structure: refuse to patch')
    text=text.replace(marker,request['pose_work']+marker,1)
    text=text.replace('_=drive_work=>{}','_=drive_work=>{},_=pose_work=>{}',1)
    files['src/robot_runner.rs']=text
files['src/robot_presence.rs']=request['module']
# All anchors checked before changing anything. Keep unrelated source intact.
for name,text in files.items():
    path=source/name; temporary=path.with_suffix('.presence-new')
    temporary.write_text(text); temporary.replace(path)
env=os.environ.copy(); env.update(CARGO_BUILD_JOBS='2',CARGO_PROFILE_DEV_DEBUG='0')
subprocess.run([str(Path.home()/'.cargo/bin/cargo'),'build','--locked','--offline','--bin','bracketbot-runner'],cwd=source,env=env,check=True)
binary=root/'bin/bracketbot-runner'; temporary=binary.with_suffix('.presence-new')
temporary.write_bytes((source/'target/debug/bracketbot-runner').read_bytes()); temporary.chmod(0o755); temporary.replace(binary)
manifest_path=root/'build-manifest.json'
manifest=json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
for path in [binary,*[source/name for name in files]]:
    manifest[str(path.relative_to(root))]=hashlib.sha256(path.read_bytes()).hexdigest()
manifest_path.write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps({'installed':str(binary),'sha256':manifest[str(binary.relative_to(root))],'preserved_existing_sources':True}))
"""


def main():
    repo=Path(__file__).resolve().parents[1]
    text=(repo/'src/robot_runner.rs').read_text()
    block='    let pose_work = async {'+text.split('    let pose_work = async {',1)[1].split('    let health = async {',1)[0]
    request=dict(module=(repo/'src/robot_presence.rs').read_text(),pose_work=block)
    subprocess.run(['bb','ssh','0184',shlex.join(['/usr/bin/python3','-c',REMOTE])],
                   input=json.dumps(request),text=True,check=True)


if __name__ == '__main__': main()
