#!/usr/bin/env python3
"""Sign a separate archive copy using existing profiles and their authorized key."""
import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import tempfile

from push_config import decode_profile
from verify_ios_package import profile_contract, verify


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--app-profile', type=Path, required=True)
    parser.add_argument('--tunnel-profile', type=Path, required=True)
    parser.add_argument('--identity', required=True, help='Authorized signing certificate SHA-1')
    parser.add_argument('--method', choices=['debugging', 'app-store-connect'], required=True)
    parser.add_argument('--build', required=True)
    parser.add_argument('--team', default='HSVMRQBQJ2')
    parser.add_argument('--group', default='group.com.peakpassvpn.ppvpn')
    parser.add_argument('--origin-file', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or not args.archive.is_dir():
        raise ValueError('archive input must exist; signed output must be new')
    if not re.fullmatch(r'[A-Fa-f0-9]{40}', args.identity):
        raise ValueError('use an explicit authorized certificate fingerprint')
    source_root = Path(__file__).resolve().parents[2] / 'apps/flutter/runner/ios'
    inputs = [('Runner', 'com.peakpassvpn.ppvpn', args.app_profile),
              ('PacketTunnel', 'com.peakpassvpn.ppvpn.tunnel', args.tunnel_profile)]
    prepared = []
    original_app = args.archive / 'Products/Applications/Runner.app'
    for target, topic, path in inputs:
        profile = decode_profile(path)
        if args.identity.upper() not in {hashlib.sha1(cert).hexdigest().upper()
                                        for cert in profile.get('DeveloperCertificates', [])}:
            raise ValueError('requested signer is not authorized by the profile')
        relative = Path('.') if target == 'Runner' else Path('PlugIns/PacketTunnel.appex')
        info = plistlib.loads((original_app / relative / 'Info.plist').read_bytes())
        ent = plistlib.loads((source_root / target / (target + '.entitlements')).read_bytes())
        permitted = profile['Entitlements']
        ent.update({'application-identifier': permitted['application-identifier'],
                    'com.apple.developer.team-identifier': args.team,
                    'get-task-allow': args.method == 'debugging',
                    'keychain-access-groups': [permitted['application-identifier']]})
        if 'beta-reports-active' in permitted:
            ent['beta-reports-active'] = permitted['beta-reports-active']
        ent.pop('aps-environment', None)
        profile_contract(info, ent, profile, topic, args.team, args.group, args.build, args.method)
        prepared.append((relative, path, ent))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(['ditto', str(args.archive), str(args.output)], capture_output=True, check=True)
    app = args.output / 'Products/Applications/Runner.app'
    with tempfile.TemporaryDirectory(prefix='ppvpn-signing-entitlements-') as work:
        def sign(bundle, entitlements=None):
            command = ['codesign', '--force', '--sign', args.identity, '--timestamp=none']
            if entitlements:
                command += ['--entitlements', str(entitlements), '--generate-entitlement-der']
            subprocess.run(command + [str(bundle)], capture_output=True, check=True)

        # All modifications precede the inside-out signing pass. Neither the
        # original archive nor the original Apple-signed profiles are edited.
        for relative, profile, entitlements in prepared:
            shutil.copyfile(profile, app / relative / 'embedded.mobileprovision')
        for library in sorted(app.rglob('*.dylib'), key=lambda p: len(p.parts), reverse=True):
            if not library.is_symlink():
                sign(library)
        for framework in sorted(app.rglob('*.framework'), key=lambda p: len(p.parts), reverse=True):
            if not framework.is_symlink():
                sign(framework)
        for index in (1, 0):
            relative, _, entitlements = prepared[index]
            path = Path(work) / (str(index) + '.plist')
            path.write_bytes(plistlib.dumps(entitlements))
            sign(app / relative, path)
    details = subprocess.run(['codesign', '-dv', '--verbose=4', str(app)],
                             capture_output=True, text=True, check=True).stderr
    authority = next(line.partition('=')[2] for line in details.splitlines() if line.startswith('Authority='))
    info_path = args.output / 'Info.plist'
    archive_info = plistlib.loads(info_path.read_bytes())
    archive_info['ApplicationProperties'].update(SigningIdentity=authority, Team=args.team)
    info_path.write_bytes(plistlib.dumps(archive_info))
    result = verify(app, args)
    result['archive_preparation'] = 'separate signed copy with source entitlements and unchanged embedded profiles'
    args.output.with_suffix('.verification.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'signed_archive': args.output.name, 'method': args.method,
                      'signature_and_profile_verified': result['signature_and_profile_verified']}))


if __name__ == '__main__':
    main()
