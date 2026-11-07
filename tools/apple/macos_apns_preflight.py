#!/usr/bin/env python3
"""Read-only signed Mac APNs candidate inventory; never launches or sends.

This is a partial preflight, not the build owner's full signing verifier or
runtime acceptance. Output intentionally cannot certify APNs delivery.
"""
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import plistlib
import subprocess


def command(args):
    try:
        result = subprocess.run(args, capture_output=True, timeout=30)
        return result.returncode, result.stdout, result.stderr
    except (OSError, subprocess.TimeoutExpired):
        return -1, b'', b''


def assess(info, entitlements, profile, signature_ok, developer_id, team):
    bundle = 'com.peakpassvpn.ppvpn.desktop'
    expected = team + '.' + bundle
    pe = profile.get('Entitlements', {})
    expires = profile.get('ExpirationDate')
    now = dt.datetime.now(dt.timezone.utc)
    if isinstance(expires, dt.datetime) and expires.tzinfo is None:
        expires = expires.replace(tzinfo=dt.timezone.utc)
    return {
        'bundle_matches': info.get('CFBundleIdentifier') == bundle,
        'explicit_push_flag_on': info.get('PPVPNApplePushEnabled') is True,
        'code_signature_verified': signature_ok,
        'developer_id_application_authority': developer_id,
        'signed_team_matches': entitlements.get('com.apple.developer.team-identifier') == team,
        'signed_app_id_matches': entitlements.get('com.apple.application-identifier') == expected,
        'signed_aps_production': entitlements.get('com.apple.developer.aps-environment') == 'production',
        'profile_team_matches': profile.get('TeamIdentifier') == [team],
        'profile_app_id_matches': pe.get('com.apple.application-identifier') == expected,
        'profile_aps_production': pe.get('com.apple.developer.aps-environment') == 'production',
        'profile_all_devices': profile.get('ProvisionsAllDevices') is True,
        'profile_not_device_limited': not profile.get('ProvisionedDevices'),
        'profile_not_expired': isinstance(expires, dt.datetime) and expires > now,
        'debug_entitlement_off': entitlements.get('get-task-allow', False) is False
            and entitlements.get('com.apple.security.get-task-allow', False) is False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', type=Path, required=True)
    parser.add_argument('--team', required=True)
    parser.add_argument('--output', type=Path, required=True, help='new private JSON path')
    args = parser.parse_args()
    app = args.app.resolve(strict=True)
    if app.suffix != '.app' or not app.is_dir():
        parser.error('--app must be an existing .app directory')
    if not (len(args.team) == 10 and args.team.isascii() and args.team.isalnum()):
        parser.error('--team must be the expected ten-character Team ID')
    # Exclusive output creation before any inspection; no overwrites or raw
    # security/codesign/process/environment output retained.
    fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as output:
        info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
        code, raw, _ = command(['/usr/bin/codesign', '-d', '--entitlements', ':-', str(app)])
        entitlements = plistlib.loads(raw) if code == 0 and raw else {}
        code, _, details = command(['/usr/bin/codesign', '-dv', '--verbose=4', str(app)])
        developer_id = code == 0 and b'Authority=Developer ID Application:' in details
        verified, _, _ = command(['/usr/bin/codesign', '--verify', '--deep', '--strict', str(app)])
        profile_path = app / 'Contents/embedded.provisionprofile'
        code, raw, _ = command(['/usr/bin/security', 'cms', '-D', '-i', str(profile_path)])
        profile = plistlib.loads(raw) if code == 0 and raw else {}
        checks = assess(info, entitlements, profile, verified == 0, developer_id, args.team)
        console_code, console_raw, _ = command(['/usr/bin/stat', '-f', '%u', '/dev/console'])
        console_matches = console_code == 0 and console_raw.strip() == str(os.getuid()).encode()
        gui_code, _, _ = command(['/bin/launchctl', 'print', 'gui/' + str(os.getuid())])
        executable = info.get('CFBundleExecutable', '')
        binary = app / 'Contents/MacOS' / executable
        # Do not follow a malformed plist outside its bundle.
        binary_hash = None
        if executable and '/' not in executable and binary.is_file() and binary.resolve().is_relative_to(app):
            with binary.open('rb') as stream:
                binary_hash = hashlib.file_digest(stream, 'sha256').hexdigest()
        report = {'scope': 'READ_ONLY_PARTIAL_PREFLIGHT', 'runtime_acceptance': 'NOT RUN',
            'app': str(app), 'build': info.get('CFBundleVersion'), 'main_executable_sha256': binary_hash,
            'checks': checks, 'blocking_checks': [key for key, value in checks.items() if not value],
            'current_uid_is_console_user': console_matches, 'gui_launchd_domain_exists': gui_code == 0,
            'existing_agent_bundled': (app / 'Contents/Library/LaunchAgents/com.peakpassvpn.ppvpn.desktop.agent.plist').exists(),
            'limits': ['No app launch, token request, permission request, login, push, service or network settings action.',
                'Console/launchd checks do not prove an unlocked GUI, notification permission or APNs availability.',
                'No credentials, Keychain contents, notification payloads, raw profiles or certificates are exported.',
                'Build owner must additionally verify certificate/profile membership, hardened runtime, nested entitlements, notarization and exact package/origin provenance.',
                'All true checks remain partial static evidence, never runtime delivery/callback/cold-tap acceptance.']}
        json.dump(report, output, indent=2)
        output.write('\n')
    print('Read-only partial preflight saved; runtime NOT RUN')


if __name__ == '__main__':
    main()
