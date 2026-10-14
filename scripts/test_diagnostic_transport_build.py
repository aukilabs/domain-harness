"""The standalone diagnostics build must not bypass Home's transport fixes."""
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]


class TransportBuildTests(unittest.TestCase):
    def test_diagnostics_and_home_use_same_patched_transport(self):
        source = 'https://github.com/aukilabs/auki-sdk.git'
        for crate in ('auki-p2p','auki-component-protocol'):
            paths = []
            for directory in (ROOT, ROOT / 'diagnostics/connection-peer'):
                manifest = tomllib.loads((directory / 'Cargo.toml').read_text())
                patch = manifest['patch'][source][crate]['path']
                paths.append((directory / patch).resolve())
                lock = tomllib.loads((directory / 'Cargo.lock').read_text())
                packages = [p for p in lock['package'] if p['name'] == crate]
                self.assertEqual(len(packages), 1)
                self.assertNotIn('source', packages[0], 'transport must resolve to local patch')
            self.assertEqual(paths[0], paths[1])


if __name__ == '__main__':
    unittest.main()
