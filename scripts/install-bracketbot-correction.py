#!/usr/bin/env python3
"""Install the laptop-built correction fix, refusing to overwrite newer changes.

No compilation or restart on the robot. Home and runtime settings stay untouched.
"""
import argparse
import json
from pathlib import Path
import shlex
import subprocess
import tarfile
import tempfile

REMOTE=r'''
import hashlib,json,subprocess,sys,tarfile
from pathlib import Path
root=Path('/home/bracketbot/deployments/domain-harness/runner')
with tarfile.open(fileobj=sys.stdin.buffer,mode='r|gz') as archive:
    contents={}
    for entry in archive:
        if not entry.isfile() or entry.name.startswith('/') or '..' in Path(entry.name).parts:
            raise SystemExit('Unexpected installer archive entry')
        contents[entry.name]=archive.extractfile(entry).read()
report=json.loads(contents['correction-build.json'])
for name,expected in report['original_sha256'].items():
    actual=hashlib.sha256((root/'source'/name).read_bytes()).hexdigest()
    if actual!=expected: raise SystemExit('Deployed source changed since build; refusing overwrite: '+name)
binary=contents['bracketbot-runner']
if hashlib.sha256(binary).hexdigest()!=report['sha256']: raise SystemExit('Binary checksum mismatch')
target=root/'bin/bracketbot-runner'; temporary=target.with_suffix('.correction-new')
temporary.write_bytes(binary); temporary.chmod(0o755)
check=subprocess.run(['ldd',str(temporary)],capture_output=True,text=True)
if check.returncode or 'not found' in check.stdout or 'not found' in check.stderr:
    raise SystemExit('Linux binary library compatibility check failed')
for name in report['changed_paths']:
    path=root/'source'/name; staging=path.with_suffix('.correction-new')
    staging.write_bytes(contents['source/'+name]); staging.chmod(path.stat().st_mode); staging.replace(path)
temporary.replace(target)
manifest_path=root/'build-manifest.json'
manifest=json.loads(manifest_path.read_text())
for path in [target,*[root/'source'/name for name in report['changed_paths']]]:
    manifest[str(path.relative_to(root))]=hashlib.sha256(path.read_bytes()).hexdigest()
manifest_path.write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps({'installed':str(target),'sha256':report['sha256'],'built_on_laptop':True,'home_untouched':True}))
'''


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--build-dir',type=Path,default=Path('/Users/nilspihl/Documents/ChatGPT/harness/data/bracketbot-linux-build'))
    args=ap.parse_args(); report_path=args.build_dir/'correction-build.json'
    report=json.loads(report_path.read_text())
    with tempfile.TemporaryFile() as bundle:
        with tarfile.open(fileobj=bundle,mode='w:gz') as archive:
            archive.add(report_path,arcname='correction-build.json')
            archive.add(report['binary'],arcname='bracketbot-runner')
            for name in report['changed_paths']:
                archive.add(args.build_dir/'source'/name,arcname='source/'+name)
        bundle.seek(0)
        subprocess.run(['bb','ssh','0184',shlex.join(['/usr/bin/python3','-c',REMOTE])],stdin=bundle,check=True)


if __name__ == '__main__': main()
