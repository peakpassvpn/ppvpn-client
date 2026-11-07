#!/usr/bin/env python3
"""Verify final iOS signatures and profiles even with push registration disabled."""
import argparse
import datetime as dt
import fnmatch
import hashlib
import json
from pathlib import Path
import plistlib
import subprocess
import tempfile

from push_config import check_contract as push_contract, decode_profile, inspect_artifact, read_signature


def profile_contract(info, signed, profile, bundle, team, group, build, method, push_enabled=False):
    def require(value, message):
        if not value:
            raise ValueError(message)

    require(info.get('CFBundleIdentifier') == bundle, 'bundle identifier differs')
    require(str(info.get('CFBundleVersion')) == build, 'build number differs')
    require(team in profile.get('TeamIdentifier', []), 'profile team differs')
    require(signed.get('com.apple.developer.team-identifier') == team, 'signed team differs')
    ent = profile.get('Entitlements', {})
    app_id = ent.get('application-identifier', '')
    require(app_id.endswith('.' + bundle) and '*' not in app_id, 'profile application identifier differs')
    require(signed.get('application-identifier') == app_id, 'signed application identifier differs')
    require(set(signed.get('com.apple.security.application-groups', [])) == {group}, 'signed App Group differs')
    require(group in ent.get('com.apple.security.application-groups', []), 'profile App Group differs')
    require(signed.get('com.apple.developer.networking.networkextension') == ['packet-tunnel-provider'],
            'signed packet-tunnel entitlement differs')
    require('packet-tunnel-provider' in ent.get('com.apple.developer.networking.networkextension', []),
            'profile lacks packet-tunnel permission')
    development = method == 'debugging'
    require(bool(signed.get('get-task-allow')) == development, 'signed development entitlement differs')
    require(bool(ent.get('get-task-allow')) == development, 'profile development entitlement differs')
    if development:
        require(bool(profile.get('ProvisionedDevices')), 'development profile has no allowed device')
    elif method == 'release-testing':
        require(bool(profile.get('ProvisionedDevices')) and not profile.get('ProvisionsAllDevices'),
                'Ad Hoc profile must authorize devices without enterprise scope')
    else:
        require(not profile.get('ProvisionedDevices') and not profile.get('ProvisionsAllDevices'),
                'App Store profile retains a device list or enterprise scope')
    expiry = profile.get('ExpirationDate')
    require(isinstance(expiry, dt.datetime) and expiry.replace(tzinfo=dt.timezone.utc) > dt.datetime.now(dt.timezone.utc),
            'profile expired or lacks expiry')
    allowed = ent.get('keychain-access-groups', [])
    for access in signed.get('keychain-access-groups', []):
        require(any(fnmatch.fnmatchcase(access, pattern) for pattern in allowed), 'keychain access group exceeds profile')
    app_push = push_enabled and bundle == 'com.peakpassvpn.ppvpn'
    if app_push:
        push_contract(info, signed, profile, 'ios', True, bundle,
                      distribution=method != 'debugging', app_store=method == 'app-store-connect')
    else:
        require(not info.get('PPVPNApplePushEnabled', False) and 'PPVPNAPNsEntitlementEnvironment' not in info,
                'push must remain disabled for this rollout or extension')
        require('aps-environment' not in signed, 'disabled push retains signed APNs entitlement')
    return {'bundle': bundle, 'team': team, 'app_group': group, 'build_number': build,
            'profile_expires': expiry.isoformat(), 'xcode_managed_profile': profile.get('IsXcodeManaged', False),
            'profile_uuid_sha256': hashlib.sha256(profile.get('UUID', '').encode()).hexdigest(),
            'get_task_allow': development, 'apns_enabled': app_push,
            'apns_environment': signed.get('aps-environment')}


def certificate_digest(bundle):
    with tempfile.TemporaryDirectory(prefix='ppvpn-public-certificate-') as work:
        prefix = Path(work) / 'certificate'
        subprocess.run(['codesign', '-d', '--extract-certificates=' + str(prefix), str(bundle)],
                       capture_output=True, check=True)
        return hashlib.sha256(Path(str(prefix) + '0').read_bytes()).hexdigest()


def verify(app, args):
    bundles = [app, app / 'PlugIns/PacketTunnel.appex']
    if list((app / 'PlugIns').glob('*.appex')) != [bundles[1]]:
        raise ValueError('unexpected app extension set')
    rows = []
    for bundle, topic in zip(bundles, ['com.peakpassvpn.ppvpn', 'com.peakpassvpn.ppvpn.tunnel']):
        info = plistlib.loads((bundle / 'Info.plist').read_bytes())
        signed = read_signature(bundle)
        profile = decode_profile(bundle / 'embedded.mobileprovision')
        row = profile_contract(info, signed, profile, topic, args.team, args.group, args.build, args.method, getattr(args, 'push_enabled', False))
        signer = certificate_digest(bundle)
        if signer not in {hashlib.sha256(cert).hexdigest() for cert in profile.get('DeveloperCertificates', [])}:
            raise ValueError('signer certificate is not authorized by embedded profile')
        row['signer_certificate_sha256'] = signer
        rows.append(row)
    if rows[0]['signer_certificate_sha256'] != rows[1]['signer_certificate_sha256']:
        raise ValueError('app and extension signers differ')
    frameworks = list((app / 'Frameworks').glob('*.framework'))
    for framework in frameworks:
        read_signature(framework)
        if certificate_digest(framework) != rows[0]['signer_certificate_sha256']:
            raise ValueError('framework signer differs from application signer')
    origin = args.origin_file.read_text().strip()
    if origin.encode() not in (app / 'Runner').read_bytes():
        raise ValueError('expected compiled backend origin not found in Runner')
    if any((app / name).exists() for name in ['data', 'logs', 'app-settings.json', 'profile.json', 'tunnel.sock']):
        raise ValueError('unexpected runtime data at package root')
    return {'signature_and_profile_verified': True, 'method': args.method, 'bundles': rows,
            'frameworks_verified': len(frameworks), 'backend_origin_sha256': hashlib.sha256(origin.encode()).hexdigest(),
            'runtime_data_at_package_root': False, 'apns_enabled': getattr(args, 'push_enabled', False),
            'note': 'Static package verification only; no device install, VPN activation, push delivery or store upload.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('artifact', type=Path)
    parser.add_argument('--method', required=True, choices=['debugging', 'release-testing', 'app-store-connect'])
    parser.add_argument('--push-enabled', action='store_true', help='Verify the main app against its actual profile APNs environment')
    parser.add_argument('--build', required=True)
    parser.add_argument('--team', default='HSVMRQBQJ2')
    parser.add_argument('--group', default='group.com.peakpassvpn.ppvpn')
    parser.add_argument('--origin-file', type=Path, required=True)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    artifact = args.artifact
    if artifact.suffix == '.xcarchive':
        apps = list((artifact / 'Products/Applications').glob('*.app'))
        if len(apps) != 1:
            raise ValueError('archive must contain exactly one app')
        artifact = apps[0]
    result = inspect_artifact(artifact, 'ios', lambda app: verify(app, args))
    text = json.dumps(result, indent=2) + '\n'
    if args.output:
        args.output.write_text(text)
    else:
        print(text, end='')


if __name__ == '__main__':
    main()
