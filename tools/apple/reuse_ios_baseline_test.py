import copy
import unittest
from reuse_ios_baseline import validate


class BaselineReuseTests(unittest.TestCase):
    def fixture(self):
        actual = {'xcode': 'Xcode fixture', 'iphoneos_sdk': 'fixture', 'rust': 'fixture',
                  'flutter': dict.fromkeys(('frameworkVersion', 'frameworkRevision', 'engineRevision', 'dartSdkVersion'), 'fixture')}
        saved = dict(actual, source_commit='source', build_number='344', backend_origin_sha256='origin',
                     cargo_lock_sha256='lock', archive_tar_sha256='archive', architecture='arm64',
                     export_contract_check_exit=0, apns_enabled=False, signing='unsigned; fixture')
        return saved, actual

    def check(self, saved, actual):
        validate(saved, actual, 'source', '344', 'origin', 'lock', 'archive')

    def test_matching_immutable_baseline_is_accepted(self):
        self.check(*self.fixture())

    def test_source_origin_archive_build_or_contract_drift_is_rejected(self):
        for key in ('source_commit', 'build_number', 'backend_origin_sha256', 'cargo_lock_sha256',
                    'archive_tar_sha256', 'architecture', 'export_contract_check_exit', 'apns_enabled', 'signing'):
            saved, actual = self.fixture()
            saved[key] = 'different'
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.check(saved, actual)

    def test_actual_sdk_and_flutter_drift_is_rejected(self):
        for key in ('xcode', 'iphoneos_sdk', 'rust'):
            saved, actual = self.fixture()
            actual[key] = 'different'
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.check(saved, actual)
        saved, actual = self.fixture()
        actual = copy.deepcopy(actual)
        actual['flutter']['engineRevision'] = 'different'
        with self.assertRaises(ValueError):
            self.check(saved, actual)


if __name__ == '__main__':
    unittest.main()
