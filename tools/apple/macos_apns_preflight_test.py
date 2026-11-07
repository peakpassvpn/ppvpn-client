"""Boundary checks preventing wrong environment/signature/profile admission."""
import copy
import datetime as dt
import unittest
from macos_apns_preflight import assess


class AdmissionTests(unittest.TestCase):
    def fixture(self):
        team = 'ABCDEFGHIJ'
        ent = {'com.apple.application-identifier': team + '.com.peakpassvpn.ppvpn.desktop',
               'com.apple.developer.team-identifier': team,
               'com.apple.developer.aps-environment': 'production'}
        profile = {'Entitlements': copy.deepcopy(ent), 'TeamIdentifier': [team],
                   'ProvisionsAllDevices': True,
                   'ExpirationDate': dt.datetime.now(dt.timezone.utc) + dt.timedelta(days=1)}
        info = {'CFBundleIdentifier': 'com.peakpassvpn.ppvpn.desktop', 'PPVPNApplePushEnabled': True}
        return info, ent, profile, True, True, team

    def test_matching_partial_static_checks(self):
        self.assertTrue(all(assess(*self.fixture()).values()))

    def test_real_signature_and_developer_id_both_required(self):
        for index in (3, 4):
            args = list(self.fixture())
            args[index] = False
            self.assertFalse(all(assess(*args).values()))

    def test_signed_production_entitlement_not_ios_key(self):
        args = self.fixture()
        args[1].pop('com.apple.developer.aps-environment')
        args[1]['aps-environment'] = 'production'
        self.assertFalse(assess(*args)['signed_aps_production'])

    def test_expired_device_limited_wrong_topic_profile_rejected(self):
        for mutation in ('expired', 'devices', 'topic'):
            args = self.fixture()
            if mutation == 'expired':
                args[2]['ExpirationDate'] = dt.datetime(2000, 1, 1)
            elif mutation == 'devices':
                args[2]['ProvisionedDevices'] = ['device']
            else:
                args[2]['Entitlements']['com.apple.application-identifier'] = 'ABCDEFGHIJ.*'
            self.assertFalse(all(assess(*args).values()))

    def test_flag_off_and_debug_entitlement_rejected(self):
        args = self.fixture()
        args[0]['PPVPNApplePushEnabled'] = False
        args[1]['com.apple.security.get-task-allow'] = True
        result = assess(*args)
        self.assertFalse(result['explicit_push_flag_on'])
        self.assertFalse(result['debug_entitlement_off'])


if __name__ == '__main__':
    unittest.main()
