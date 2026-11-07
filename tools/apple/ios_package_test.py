import copy
import datetime as dt
import unittest

from verify_ios_package import profile_contract


class IOSPackageContractTests(unittest.TestCase):
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


if __name__ == '__main__':
    unittest.main()
