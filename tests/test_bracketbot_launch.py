import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('launch', Path(__file__).resolve().parents[1] / 'scripts/run-bracketbot-runner.py')
launch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launch)


class LaunchPreflightTests(unittest.TestCase):
    def test_missing_or_loopback_home_fails_before_stopping_runner(self):
        for home in [None, dict(schema='spatial-harness.home-connection/v1', peer_id='home',
                               frame='home', product_id='canonical-scene',
                               routes=['/ip4/127.0.0.1/tcp/4322'])]:
            with self.subTest(home=home), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root/'bin').mkdir()
                (root/'bin/bracketbot-runner').touch()
                (root/'state').mkdir()
                # A malformed PID would fail if the preflight proceeded to stop.
                (root/'state/runner.pid').write_text('must not inspect this PID')
                result = subprocess.run([sys.executable, '-c', launch.REMOTE],
                    input=json.dumps(dict(root=directory,home=home,restart=True)),
                    text=True,capture_output=True)
                self.assertNotEqual(result.returncode,0)
                self.assertIn('Home connection',result.stderr)
                self.assertNotIn('Traceback',result.stderr)
                self.assertEqual((root/'state/runner.pid').read_text(),'must not inspect this PID')


if __name__ == '__main__': unittest.main()
