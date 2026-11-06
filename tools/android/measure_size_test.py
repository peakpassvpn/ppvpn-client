import copy
from pathlib import Path
import struct
import tempfile
import unittest
import zipfile

import measure_size as size


class AndroidSizeTests(unittest.TestCase):
    def fixture(self, path, prefix=''):
        header = bytearray(20)
        header[:6] = b'\x7fELF\x02\x01'
        struct.pack_into('<H', header, 18, 183)
        with zipfile.ZipFile(path, 'w') as z:
            z.writestr(prefix + 'lib/arm64-v8a/libbridge.so', header)
            z.writestr(prefix + 'classes.dex', b'dex\n035\x00')
        return dict.fromkeys(size.REQUIRED, 'same')

    def test_apk_and_aab_are_distinct_and_do_not_claim_play_download_size(self):
        with tempfile.TemporaryDirectory() as work:
            path = Path(work) / 'app.apk'
            conditions = self.fixture(path)
            apk = size.inventory(path, 'apk', 'abi-split', ['arm64-v8a'], 'a', 'base', conditions)
            self.assertEqual(apk['components']['native/arm64-v8a'], 20)
            self.assertEqual(apk['actual_abis'], ['arm64-v8a'])
            self.assertIsNone(apk['play_device_download_bytes'])
            self.fixture(path, 'base/')
            aab = size.inventory(path, 'aab', 'bundle', ['arm64-v8a'], 'b', 'base', conditions)
            with self.assertRaises(ValueError):
                size.compare(apk, aab)

    def test_abi_and_optimization_mismatches_fail_closed(self):
        with tempfile.TemporaryDirectory() as work:
            path = Path(work) / 'app.apk'
            conditions = self.fixture(path)
            with self.assertRaises(ValueError):
                size.inventory(path, 'apk', 'abi-split', ['x86_64'], 'a', 'base', conditions)
            a = size.inventory(path, 'apk', 'abi-split', ['arm64-v8a'], 'a', 'base', conditions)
            b = copy.deepcopy(a)
            b['conditions']['optimization'] = 'debug'
            with self.assertRaises(ValueError):
                size.compare(a, b)

    def test_signed_container_bytes_and_feature_changes_stay_visible(self):
        with tempfile.TemporaryDirectory() as work:
            path = Path(work) / 'app.apk'
            conditions = self.fixture(path)
            a = size.inventory(path, 'apk', 'abi-split', ['arm64-v8a'], 'a', 'base', conditions)
            b = copy.deepcopy(a)
            b['container_bytes'] -= 100
            b['approved_removals'] = ['mobile-local-proxy-server']
            result = size.compare(a, b)
            self.assertEqual(result['saved_bytes'], 100)
            self.assertEqual(result['candidate_approved_removals'], ['mobile-local-proxy-server'])


if __name__ == '__main__':
    unittest.main()
