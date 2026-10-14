import importlib.util
import io
import json
from pathlib import Path
import struct
import unittest

spec = importlib.util.spec_from_file_location('camera_adapter', Path(__file__).parents[1] / 'scripts/diagnostic-camera.py')
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)

class BinaryCameraIPC(unittest.TestCase):
    def test_exact_bytes_and_capture_timestamp(self):
        output = io.BytesIO()
        adapter.write_configuration({'width': 640}, output)
        stamp = 9007199254740993
        jpeg = bytes([255, 216, 0, 10, 34, 255, 217])
        adapter.write_frame(stamp, jpeg, output)
        data = output.getvalue()
        size, = struct.unpack_from('<I', data)
        self.assertEqual(json.loads(data[4:4+size]), {'width': 640})
        self.assertEqual(struct.unpack_from('<QI', data, 4+size), (stamp, len(jpeg)))
        self.assertEqual(data[4+size+12:], jpeg)

    def test_rejects_unbounded_and_empty_frames(self):
        for value in (b'', bytes(2*1024*1024+1)):
            with self.assertRaises(ValueError):
                adapter.write_frame(100, value, io.BytesIO())

if __name__ == '__main__':
    unittest.main()
