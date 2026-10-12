"""Exercise the incremental installer without contacting or restarting hardware."""
import contextlib
import io
import json
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest
from unittest.mock import patch


class PresenceDeployTests(unittest.TestCase):
    def test_existing_sources_configuration_and_motion_scripts_are_preserved(self):
        repo=Path(__file__).resolve().parents[1]
        remote=runpy.run_path(str(repo/'scripts/build-bracketbot-presence.py'))['REMOTE']
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); source=root/'source'; (source/'src').mkdir(parents=True)
            for name in ('lib.rs','mobile_pose.rs','robot_runner.rs'):
                text=subprocess.check_output(['git','show','HEAD:src/'+name],cwd=repo,text=True)
                (source/'src'/name).write_text(text+'\n// existing colleague changes preserved\n')
            untouched=source/'src/local_mapper.rs'; untouched.write_text('existing joint telemetry')
            profile=root/'home-connection.json'; profile.write_text('existing profile')
            scripts=source/'scripts'; scripts.mkdir(); motion=scripts/'supervise-bracketbot.py'
            motion.write_text('existing phone drive settings')
            (source/'target/debug').mkdir(parents=True)
            (source/'target/debug/bracketbot-runner').write_bytes(b'tested-binary')
            (root/'bin').mkdir()
            text=(repo/'src/robot_runner.rs').read_text()
            block='    let pose_work = async {'+text.split('    let pose_work = async {',1)[1].split('    let health = async {',1)[0]
            request=json.dumps(dict(module=(repo/'src/robot_presence.rs').read_text(),pose_work=block))
            program=remote.replace("Path('/home/bracketbot/deployments/domain-harness/runner')",'Path(TEST_ROOT)')
            with patch('sys.stdin',io.StringIO(request)),patch('subprocess.run') as build,contextlib.redirect_stdout(io.StringIO()):
                exec(compile(program,'presence-installer','exec'),{'TEST_ROOT':directory})
            self.assertEqual(build.call_count,1)
            self.assertEqual(untouched.read_text(),'existing joint telemetry')
            self.assertEqual(profile.read_text(),'existing profile')
            self.assertEqual(motion.read_text(),'existing phone drive settings')
            for name in ('lib.rs','mobile_pose.rs','robot_runner.rs'):
                self.assertIn('// existing colleague changes preserved',(source/'src'/name).read_text())
            self.assertIn('pose_work',(source/'src/robot_runner.rs').read_text())
            self.assertIn('send_component',(source/'src/mobile_pose.rs').read_text())
            self.assertEqual((root/'bin/bracketbot-runner').read_bytes(),b'tested-binary')


if __name__ == '__main__': unittest.main()
