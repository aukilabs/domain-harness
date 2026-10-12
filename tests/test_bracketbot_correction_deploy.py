import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('correction_build',REPO/'scripts/build-bracketbot-correction.py')
builder=importlib.util.module_from_spec(spec);spec.loader.exec_module(builder)


class CorrectionDeployTests(unittest.TestCase):
    def test_patch_is_incremental_preserves_colleague_code_and_is_repeatable(self):
        names=['src/local_mapper.rs','src/recent_voxels.rs','src/robot_live.rs','src/fresh_ingress.rs','src/robot_runner.rs',
               'scripts/bracketbot_capture.py','scripts/capture-bracketbot.py','src/frame_transform.rs',
               'scripts/bracketbot_pointing.py','scripts/bracketbot_note.py','scripts/bracketbot_volume_speech.py']
        with tempfile.TemporaryDirectory() as directory:
            source=Path(directory)
            for name in names:
                path=source/name;path.parent.mkdir(exist_ok=True)
                text=subprocess.check_output(['git','show','HEAD:'+name],cwd=REPO,text=True)
                marker='\n# colleague change preserved\n' if name.endswith('.py') else '\n// colleague change preserved\n'
                path.write_text(text+marker)
            unchanged=source/'src/mobile_pose.rs';unchanged.write_text('existing phone naming and robot presence')
            self.assertEqual(set(builder.patch_sources(source,REPO)),set(names))
            before={name:(source/name).read_text() for name in names}
            builder.patch_sources(source,REPO)
            for name in names:
                self.assertIn('colleague change preserved',(source/name).read_text())
                self.assertEqual((source/name).read_text(),before[name])
            self.assertEqual(unchanged.read_text(),'existing phone naming and robot presence')
            self.assertIn('base_odometry_pose',(source/'src/local_mapper.rs').read_text())
            self.assertNotIn('correction=trajectory_correction',(source/'scripts/capture-bracketbot.py').read_text())
            self.assertIn('def point_in_frame(', (source/'scripts/bracketbot_pointing.py').read_text())


if __name__=='__main__':unittest.main()
