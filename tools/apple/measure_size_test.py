import io
import json
from pathlib import Path
import struct
import tempfile
import unittest
import zipfile
from unittest.mock import patch
from argparse import Namespace

import measure_size as size


class InventoryTests(unittest.TestCase):
    def test_macho_architecture_and_universal_slices(self):
        thin = struct.pack('<IIII', 0xfeedfacf, 0x0100000c, 0, 6)
        self.assertEqual(size.macho(thin)['architectures'], ['arm64'])
        fat = struct.pack('>II', 0xcafebabe, 2)
        fat += struct.pack('>IIIII', 0x0100000c, 0, 100, 8, 0)
        fat += struct.pack('>IIIII', 0x01000007, 0, 200, 8, 0)
        self.assertEqual(size.macho(fat)['architectures'], ['arm64', 'x86_64'])
        with self.assertRaises(ValueError):
            size.macho(struct.pack('>II', 0xcafebabe, 65))

    def test_framework_symlinks_do_not_duplicate_or_escape_artifact(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work) / 'App.app'
            version = root / 'Contents/Frameworks/Test.framework/Versions/A'
            version.mkdir(parents=True)
            (version / 'Test').write_bytes(b'1234')
            (version.parent / 'Current').symlink_to('A')
            (root / 'escape').symlink_to(Path(work))
            rows, links = size.directory_rows(root)
            self.assertEqual(size.summarize(rows, links)['regular_file_bytes'], 4)
            self.assertEqual(len(links), 2)

    def test_extension_and_shared_library_are_separate_components(self):
        row = size.file_row('PlugIns/Tunnel.appex/Tunnel',
                            io.BytesIO(struct.pack('<IIII', 0xfeedfacf, 0x0100000c, 0, 2)))
        self.assertEqual((row['owner'], row['category']), ('extension:Tunnel.appex', 'program'))
        dylib = size.file_row('Frameworks/Bridge.dylib',
                              io.BytesIO(struct.pack('<IIII', 0xfeedfacf, 0x0100000c, 0, 6)))
        self.assertEqual(dylib['category'], 'shared_library')

    def test_ipa_reports_compressed_container_without_claiming_device_download_size(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            artifact = root / 'App.ipa'
            with zipfile.ZipFile(artifact, 'w', compression=zipfile.ZIP_DEFLATED) as z:
                z.writestr('Payload/App.app/Runner', b'a' * 10000)
                z.writestr('Payload/App.app/PlugIns/Tunnel.appex/Tunnel', b'b' * 20)
            conditions = root / 'conditions.json'
            conditions.write_text(json.dumps(dict.fromkeys(
                ['architecture', 'optimization', 'strip', 'signing', 'toolchain', 'build_number'], 'same')))
            result = size.inventory(Namespace(artifact=artifact, conditions=conditions,
                kind='ipa', platform='ios', source_commit='candidate', functional_baseline='baseline'))
            self.assertEqual(result['application']['regular_file_bytes'], 10020)
            self.assertLess(result['container']['bytes'], 10020)
            self.assertIsNone(result['device_download_size'])
            with zipfile.ZipFile(artifact, 'w') as z:
                z.writestr('../escape', b'x')
            with self.assertRaises(ValueError):
                size.inventory(Namespace(artifact=artifact, conditions=conditions,
                    kind='ipa', platform='ios', source_commit='candidate', functional_baseline='baseline'))

    def test_archive_keeps_debug_symbols_out_of_application_bytes(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            archive = root / 'App.xcarchive'
            app = archive / 'Products/Applications/App.app'
            app.mkdir(parents=True)
            (app / 'Runner').write_bytes(b'a' * 100)
            debug = archive / 'dSYMs/App.app.dSYM'
            debug.mkdir(parents=True)
            (debug / 'symbols').write_bytes(b'b' * 500)
            conditions = root / 'conditions.json'
            conditions.write_text(json.dumps(dict.fromkeys(
                ['architecture', 'optimization', 'strip', 'signing', 'toolchain', 'build_number'], 'same')))
            result = size.inventory(Namespace(artifact=archive, conditions=conditions,
                kind='xcarchive', platform='ios', source_commit='candidate', functional_baseline='baseline'))
            self.assertEqual(result['application']['regular_file_bytes'], 100)
            self.assertEqual(result['artifact']['regular_file_bytes'], 600)
            comparison = size.compare(result, result)
            self.assertEqual(comparison['savings']['artifact']['saved_bytes'], 0)

    def test_abi_fails_when_required_export_is_missing(self):
        args = Namespace(baseline_library=Path('before'), candidate_library=Path('after'),
                         required_symbols=None, baseline_binding=None, candidate_binding=None)
        with patch.object(size, 'exports', side_effect=[{'frb_one', 'frb_two'}, {'frb_one'}]):
            result = size.abi(args)
        self.assertFalse(result['compatible_exports'])
        self.assertEqual(result['missing'], ['frb_two'])

    def test_compare_refuses_different_signing_architecture_and_measurement_kind(self):
        base = {'schema': 1, 'platform': 'macos', 'kind': 'app', 'source_commit': 'base',
                'functional_baseline': 'same', 'conditions': {**dict.fromkeys(
                    ['architecture', 'optimization', 'strip', 'toolchain', 'build_number'], 'same'),
                    'signing': 'ad-hoc'},
                'application': {'regular_file_bytes': 2000000, 'architectures': ['arm64'], 'components': {}}}
        after = json.loads(json.dumps(base))
        after['application']['regular_file_bytes'] = 1000000
        delta = size.compare(base, after)['savings']['application']
        self.assertEqual(delta['saved_MB'], 1)
        self.assertAlmostEqual(delta['saved_MiB'], 0.9536743164)
        for key, value in [('kind', 'ipa'), ('conditions', {'signing': 'Developer ID'})]:
            bad = {**after, key: value}
            with self.assertRaises(ValueError):
                size.compare(base, bad)
        with self.assertRaises(ValueError):
            size.compare(base, {**after, 'conditions': {'toolchain': None}})
        after['application']['architectures'] = ['x86_64']
        with self.assertRaises(ValueError):
            size.compare(base, after)


if __name__ == '__main__':
    unittest.main()
