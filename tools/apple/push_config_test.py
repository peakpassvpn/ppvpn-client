from datetime import datetime, timedelta, timezone
from pathlib import Path
import plistlib
import tempfile
import unittest
from unittest.mock import patch

import push_config as push


class PushContractTests(unittest.TestCase):
    def profile(self, platform='ios', environment='production'):
        return {'TeamIdentifier': ['TESTTEAM00'], 'ApplicationIdentifierPrefix': ['TESTTEAM00'],
                'ExpirationDate': datetime.now(timezone.utc) + timedelta(days=1),
                'Entitlements': {push.ENV_KEYS[platform]: environment,
                    push.APP_KEYS[platform]: 'TESTTEAM00.com.example.app', 'get-task-allow': False,
                    'com.apple.developer.team-identifier': 'TESTTEAM00'}}

    def info(self, environment='production'):
        return {'CFBundleIdentifier': 'com.example.app', 'PPVPNApplePushEnabled': True,
                'PPVPNAPNsEntitlementEnvironment': environment}

    def signed(self, platform='ios', environment='production'):
        return {push.ENV_KEYS[platform]: environment,
                push.APP_KEYS[platform]: 'TESTTEAM00.com.example.app',
                'com.apple.developer.team-identifier': 'TESTTEAM00'}

    def test_production_and_development_come_from_actual_profile(self):
        for platform in ('ios', 'macos'):
            for environment in ('production', 'development'):
                row = push.check_contract(self.info(environment), self.signed(platform, environment),
                    self.profile(platform, environment), platform, True, 'com.example.app')
                self.assertEqual(row['environment'], environment)

    def test_export_or_resign_environment_change_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'environments differ'):
            push.check_contract(self.info('development'), self.signed(), self.profile(),
                                'ios', True, 'com.example.app', distribution=True)

    def test_distribution_rejects_sandbox_or_development_profile(self):
        for environment in ('development', 'production'):
            profile = self.profile(environment=environment)
            if environment == 'production':
                profile['Entitlements']['get-task-allow'] = True
            with self.assertRaisesRegex(ValueError, 'distribution'):
                push.check_contract(self.info(environment), self.signed(environment=environment),
                    profile, 'ios', True, 'com.example.app', distribution=True)

    def test_legacy_app_id_prefix_is_not_mistaken_for_team_identity(self):
        profile = self.profile()
        profile['ApplicationIdentifierPrefix'] = ['LEGACYP000']
        profile['Entitlements']['application-identifier'] = 'LEGACYP000.com.example.app'
        signed = self.signed()
        signed['application-identifier'] = 'LEGACYP000.com.example.app'
        self.assertTrue(push.check_contract(self.info(), signed, profile, 'ios', True,
                                            'com.example.app', distribution=True)['enabled'])

    def test_ad_hoc_devices_are_allowed_but_not_labeled_app_store(self):
        profile = self.profile()
        profile['ProvisionedDevices'] = ['synthetic-device']
        self.assertTrue(push.check_contract(self.info(), self.signed(), profile, 'ios', True,
                                            'com.example.app', distribution=True)['enabled'])
        with self.assertRaisesRegex(ValueError, 'App Store'):
            push.check_contract(self.info(), self.signed(), profile, 'ios', True,
                                'com.example.app', distribution=True, app_store=True)

    def test_wrong_topic_wildcard_expired_and_wrong_team_are_rejected(self):
        for change in ('wildcard', 'expired', 'team'):
            profile = self.profile()
            if change == 'wildcard':
                profile['Entitlements']['application-identifier'] = 'TESTTEAM00.*'
            elif change == 'expired':
                profile['ExpirationDate'] = datetime(2000, 1, 1, tzinfo=timezone.utc)
            else:
                profile['TeamIdentifier'] = ['OTHERT0000']
            with self.assertRaises(ValueError):
                push.profile_contract(profile, 'ios', 'com.example.app')
        with self.assertRaises(ValueError):
            push.check_contract(self.info(), self.signed(), self.profile(), 'ios', True, 'com.other.app')
        signed = self.signed()
        signed['com.apple.developer.team-identifier'] = 'OTHERT0000'
        with self.assertRaisesRegex(ValueError, 'signed team'):
            push.check_contract(self.info(), signed, self.profile(), 'ios', True, 'com.example.app')

    def test_default_disabled_build_has_no_environment_stamp(self):
        info = {'CFBundleIdentifier': 'com.example.app', 'PPVPNApplePushEnabled': False}
        self.assertFalse(push.check_contract(info, {}, {}, 'ios', False, 'com.example.app')['enabled'])
        info['PPVPNAPNsEntitlementEnvironment'] = 'production'
        with self.assertRaises(ValueError):
            push.check_contract(info, {}, {}, 'ios', False, 'com.example.app')
        info['PPVPNApplePushEnabled'] = 'false'
        with self.assertRaises(ValueError):
            push.check_contract(info, {}, {}, 'ios', False, 'com.example.app')

    def test_prepare_requires_real_profile_and_preserves_other_entitlements(self):
        with tempfile.TemporaryDirectory() as directory:
            info, ent = Path(directory) / 'Info.plist', Path(directory) / 'Runner.entitlements'
            initial = {'CFBundleIdentifier': '$(PRODUCT_BUNDLE_IDENTIFIER)'}
            info.write_bytes(plistlib.dumps(initial))
            ent.write_bytes(plistlib.dumps({'application-groups': ['group.example'], 'aps-environment': 'development'}))
            with self.assertRaises(ValueError):
                push.prepare(info, [ent], 'ios', True, None, 'com.example.app')
            self.assertEqual(plistlib.loads(info.read_bytes()), initial)
            with patch.object(push, 'decode_profile', return_value=self.profile()):
                push.prepare(info, [ent], 'ios', True, Path('synthetic.mobileprovision'), 'com.example.app')
            self.assertEqual(plistlib.loads(info.read_bytes())['PPVPNAPNsEntitlementEnvironment'], 'production')
            self.assertEqual(plistlib.loads(ent.read_bytes())['application-groups'], ['group.example'])
            push.prepare(info, [ent], 'ios', False, None, 'com.example.app')
            self.assertNotIn('aps-environment', plistlib.loads(ent.read_bytes()))

    def test_final_macos_identity_is_profile_authorized_and_atomic(self):
        with tempfile.TemporaryDirectory() as directory:
            info, ent = Path(directory) / 'Info.plist', Path(directory) / 'Entitlements.plist'
            profile = self.profile('macos')
            profile['Entitlements']['keychain-access-groups'] = ['TESTTEAM00.*']
            for conflict in (None, 'wrong-keychain', 'missing-authorization', 'wrong-identity'):
                original = {'com.apple.security.network.client': True}
                if conflict == 'wrong-keychain':
                    original['keychain-access-groups'] = ['TESTTEAM00.other']
                if conflict == 'wrong-identity':
                    original[push.APP_KEYS['macos']] = 'OTHER.com.example.app'
                profile['Entitlements']['keychain-access-groups'] = ([] if conflict == 'missing-authorization' else ['TESTTEAM00.*'])
                info.write_bytes(plistlib.dumps({'CFBundleIdentifier': 'com.example.app'}))
                ent.write_bytes(plistlib.dumps(original))
                before = (info.read_bytes(), ent.read_bytes())
                with patch.object(push, 'decode_profile', return_value=profile):
                    if conflict:
                        with self.assertRaises(ValueError):
                            push.prepare(info, [ent], 'macos', True, Path('fixture'), 'com.example.app', True)
                        self.assertEqual((info.read_bytes(), ent.read_bytes()), before)
                    else:
                        push.prepare(info, [ent], 'macos', True, Path('fixture'), 'com.example.app', True)
                        actual = plistlib.loads(ent.read_bytes())
                        self.assertEqual(actual['keychain-access-groups'], ['TESTTEAM00.com.example.app'])
                        self.assertEqual(actual[push.APP_KEYS['macos']], 'TESTTEAM00.com.example.app')
                        self.assertTrue(actual['com.apple.security.network.client'])

    def test_final_signer_must_be_authorized_by_profile(self):
        def extract(command, **kwargs):
            prefix = next(item.split('=', 1)[1] for item in command if item.startswith('--extract-certificates='))
            Path(prefix + '0').write_bytes(b'synthetic-public-certificate')
        with patch.object(push.subprocess, 'run', side_effect=extract):
            digest = push.require_profile_signer(Path('fixture.app'), {'DeveloperCertificates': [b'synthetic-public-certificate']})
            self.assertEqual(len(digest), 64)
            with self.assertRaisesRegex(ValueError, 'not authorized'):
                push.require_profile_signer(Path('fixture.app'), {'DeveloperCertificates': [b'other-certificate']})

    def test_sources_are_idempotent_and_remove_only_identical_generated_copy(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            common = root / 'runner/apple/ApplePushState.swift'
            native = root / 'runner/ios/Runner/ApplePush.swift'
            entry = root / 'ios/Runner/AppDelegate.swift'
            for path in (common, native, entry):
                path.parent.mkdir(parents=True, exist_ok=True)
            common.write_text('class State {}\n')
            native.write_text('class Receiver {}\n')
            with self.assertRaisesRegex(ValueError, 'owned runner'):
                push.include_sources(root, 'ios', native)
            entry.write_text('class AppDelegate {}\n')
            copied = entry.parent / 'ApplePush.swift'
            copied.write_text(native.read_text())
            push.include_sources(root, 'ios', entry)
            first = entry.read_text()
            self.assertFalse(copied.exists())
            push.include_sources(root, 'ios', entry)
            self.assertEqual(first, entry.read_text())
            self.assertEqual(first.count('class State'), 1)
            copied.write_text('unknown generated changes')
            with self.assertRaises(ValueError):
                push.include_sources(root, 'ios', entry)

    def test_unsigned_mode_cannot_bypass_enabled_or_distribution_checks(self):
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory)
            (app / 'Info.plist').write_bytes(plistlib.dumps(self.info()))
            with self.assertRaises(ValueError):
                push.verify_bundle(app, 'ios', True, 'com.example.app', allow_unsigned_disabled=True)
            with self.assertRaises(ValueError):
                push.verify_bundle(app, 'ios', False, 'com.example.app', distribution=True,
                                   allow_unsigned_disabled=True)


if __name__ == '__main__':
    unittest.main()
