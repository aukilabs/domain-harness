#!/usr/bin/env python3
"""Bundle pinned sources, build on the ARM64 robot, and install the runner.

Requires bb SSH access and the robot's existing Rust toolchain. No credentials
or data directory are transferred. Use --code-only after the first deployment.
Starting/restarting the runner is a separate run-bracketbot-runner.py command.
"""
import argparse
import json
from pathlib import Path
import shlex
import subprocess
import tarfile
import tempfile


REMOTE = r'''
import hashlib,json,os,subprocess,sys,tarfile
from pathlib import Path
root=Path(sys.argv[1]); source=root/'source'
source.mkdir(parents=True,exist_ok=True)
with tarfile.open(fileobj=sys.stdin.buffer,mode='r|gz') as archive:
    for entry in archive:
        resolved=(source/entry.name).resolve()
        if not resolved.is_relative_to(source.resolve()) or entry.issym() or entry.islnk():
            raise SystemExit('Unsafe source archive member')
        archive.extract(entry,source)
(source/'.cargo').mkdir(exist_ok=True)
(source/'.cargo/config.toml').write_bytes((source/'.deps/bracketbot-build-config/config.toml').read_bytes())
env=os.environ.copy(); env.update(CARGO_BUILD_JOBS='2',CARGO_PROFILE_DEV_DEBUG='0')
subprocess.run([str(Path.home()/'.cargo/bin/cargo'),'build','--locked','--offline',
    '--bin','bracketbot-runner'],cwd=source,env=env,check=True)
binary=root/'bin/bracketbot-runner'; binary.parent.mkdir(exist_ok=True)
tmp=binary.with_suffix('.new'); tmp.write_bytes((source/'target/debug/bracketbot-runner').read_bytes())
tmp.chmod(0o755); tmp.replace(binary)
files=[binary,source/'Cargo.lock',*sorted((source/'scripts').glob('*bracketbot*.py'))]
manifest={str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files}
(root/'build-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps({'installed':str(binary),'sha256':manifest[str(binary.relative_to(root))]}))
'''


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--robot', default='0184')
    ap.add_argument('--root', default='/home/bracketbot/deployments/domain-harness/runner')
    ap.add_argument('--code-only', action='store_true', help='Reuse dependencies already vendored on the robot')
    args = ap.parse_args()
    if not Path(args.root).is_absolute(): ap.error('--root must be absolute')
    repo = Path(__file__).resolve().parents[1]
    paths = ['Cargo.toml', 'Cargo.lock', 'src', 'config', 'scripts', 'web',
             'ios/PortalBridge/Cargo.toml', 'ios/PortalBridge/src',
             'vendor/auki-p2p', 'vendor/auki-component-protocol', 'vendor/auki-voxel-map']
    if not args.code_only:
        config = repo/'.deps/bracketbot-build-config/config.toml'
        config.parent.mkdir(parents=True, exist_ok=True)
        with config.open('w') as output:
            subprocess.run(['cargo', 'vendor', '--locked', '--versioned-dirs',
                            '.deps/bracketbot-vendor'], cwd=repo, stdout=output, check=True)
        paths += ['.deps/bracketbot-build-config', '.deps/bracketbot-vendor']
    def include(entry):
        # Dependency source trees can legitimately contain a directory "target".
        if any(part in ('__pycache__', '.git', '.DS_Store') for part in Path(entry.name).parts): return None
        if entry.issym() or entry.islnk(): raise ValueError('Source bundle must contain regular files/directories')
        return entry
    with tempfile.TemporaryFile() as bundle:
        with tarfile.open(fileobj=bundle, mode='w:gz') as archive:
            for path in paths: archive.add(repo/path, arcname=path, filter=include)
        bundle.seek(0)
        subprocess.run(['bb', 'ssh', args.robot,
                        shlex.join(['/usr/bin/python3', '-c', REMOTE, args.root])], stdin=bundle, check=True)


if __name__ == '__main__': main()
