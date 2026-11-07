import copy
import datetime as dt
import unittest
from pathlib import Path
import tempfile

from verify_ios_package import profile_contract, require_selected_profile


class IOSPackageContractTests(unittest.TestCase):
    def test_export_cannot_replace_selected_profile_with_another_valid_profile(self):
        with tempfile.TemporaryDirectory() as directory:
            expected, embedded = Path(directory) / 'approved', Path(directory) / 'embedded'
            expected.write_bytes(b'fresh Apple profile')
            embedded.write_bytes(expected.read_bytes())
            self.assertTrue(require_selected_profile(embedded, expected))
            embedded.write_bytes(b'older cached Apple profile')
            with self.assertRaisesRegex(ValueError, 'replaced the explicitly selected'):
                require_selected_profile(embedded, expected)

    def fixture(self):
        bundle = 'com.peakpassvpn.ppvpn'
        ent = {'application-identifier': 'OLDPREFIX.' + bundle,
               'com.apple.developer.team-identifier': 'TEAM',
               'com.apple.security.application-groups': ['group.product'],
               'com.apple.developer.networking.networkextension': ['packet-tunnel-provider'],
               'get-task-allow': False, 'keychain-access-groups': ['OLDPREFIX.' + bundle]}
        profile = {'UUID': 'fixture', 'TeamIdentifier': ['TEAM'],
                   'ExpirationDate': dt.datetime(2099, 1, 1), 'Entitlements': copy.deepcopy(ent)}
        profile['Entitlements']['keychain-access-groups'] = ['OLDPREFIX.*']
        return {'CFBundleIdentifier': bundle, 'CFBundleVersion': '344'}, ent, profile

    def check(self, info, ent, profile, method='app-store-connect'):
        return profile_contract(info, ent, profile, 'com.peakpassvpn.ppvpn', 'TEAM',
                                'group.product', '344', method)

    def test_actual_profile_prefix_may_differ_from_team(self):
        info, ent, profile = self.fixture()
        self.assertEqual(self.check(info, ent, profile)['team'], 'TEAM')

    def test_missing_or_diagnostic_app_group_is_rejected(self):
        for groups in ([], ['group.product', 'group.diagnostic']):
            info, ent, profile = self.fixture()
            ent['com.apple.security.application-groups'] = groups
            with self.assertRaisesRegex(ValueError, 'App Group'):
                self.check(info, ent, profile)

    def test_development_permissions_cannot_pass_as_app_store(self):
        info, ent, profile = self.fixture()
        ent['get-task-allow'] = profile['Entitlements']['get-task-allow'] = True
        profile['ProvisionedDevices'] = ['fixture-device']
        self.check(info, ent, profile, 'debugging')
        with self.assertRaises(ValueError):
            self.check(info, ent, profile)

    def test_ad_hoc_production_push_is_device_limited_and_not_store(self):
        info, ent, profile = self.fixture()
        profile['ApplicationIdentifierPrefix'] = ['OLDPREFIX']
        profile['ProvisionedDevices'] = ['fixture-device']
        info.update(PPVPNApplePushEnabled=True, PPVPNAPNsEntitlementEnvironment='production')
        ent['aps-environment'] = profile['Entitlements']['aps-environment'] = 'production'
        row = profile_contract(info, ent, profile, 'com.peakpassvpn.ppvpn', 'TEAM',
                               'group.product', '344', 'release-testing', True)
        self.assertTrue(row['apns_enabled'])
        with self.assertRaisesRegex(ValueError, 'App Store'):
            profile_contract(info, ent, profile, 'com.peakpassvpn.ppvpn', 'TEAM',
                             'group.product', '344', 'app-store-connect', True)

    def test_production_distribution_rejects_sandbox_push(self):
        info, ent, profile = self.fixture()
        profile['ApplicationIdentifierPrefix'] = ['OLDPREFIX']
        profile['ProvisionedDevices'] = ['fixture-device']
        info.update(PPVPNApplePushEnabled=True, PPVPNAPNsEntitlementEnvironment='development')
        ent['aps-environment'] = profile['Entitlements']['aps-environment'] = 'development'
        with self.assertRaisesRegex(ValueError, 'production APNs'):
            profile_contract(info, ent, profile, 'com.peakpassvpn.ppvpn', 'TEAM',
                             'group.product', '344', 'release-testing', True)

    def test_push_must_be_explicit_and_authorized_by_profile(self):
        info, ent, profile = self.fixture()
        profile['ApplicationIdentifierPrefix'] = ['OLDPREFIX']
        info.update(PPVPNApplePushEnabled=True, PPVPNAPNsEntitlementEnvironment='production')
        ent['aps-environment'] = 'production'
        with self.assertRaises(ValueError):
            self.check(info, ent, profile)
        with self.assertRaises(ValueError):
            profile_contract(info, ent, profile, 'com.peakpassvpn.ppvpn', 'TEAM',
                             'group.product', '344', 'app-store-connect', True)


if __name__ == '__main__':
    unittest.main()
