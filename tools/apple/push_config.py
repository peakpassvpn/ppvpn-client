#!/usr/bin/env python3
"""Configure Apple push builds and verify actual exported/re-signed bundles.

Never registers with APNs, installs profiles, signs, or modifies signed products.
Profile contents and codesign diagnostics are captured and never printed.
"""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path, PurePosixPath
import plistlib
import shutil
import stat
import subprocess
import tempfile
import zipfile

ENV_KEYS = {'ios': 'aps-environment', 'macos': 'com.apple.developer.aps-environment'}
APP_KEYS = {'ios': 'application-identifier', 'macos': 'com.apple.application-identifier'}
MARKER = '// BEGIN PPVPN GENERATED APPLE PUSH SOURCES'
END_MARKER = '// END PPVPN GENERATED APPLE PUSH SOURCES'


class ConfigurationError(ValueError):
    pass


def decode_profile(path):
    try:
        result = subprocess.run(['security', 'cms', '-D', '-i', str(path)],
                                capture_output=True, check=True)
        return plistlib.loads(result.stdout)
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        raise ConfigurationError('provision could not be decoded') from error


def profile_contract(profile, platform, topic, now=None):
    if not isinstance(profile, dict) or not isinstance(profile.get('Entitlements'), dict):
        raise ConfigurationError('provision entitlements are not a dictionary')
    entitlements = profile['Entitlements']
    environment = entitlements.get(ENV_KEYS[platform])
    if environment not in ('development', 'production'):
        raise ConfigurationError('provision lacks a supported APNs environment')
    teams = profile.get('TeamIdentifier', [])
    if not isinstance(teams, list) or len(teams) != 1 or not isinstance(teams[0], str):
        raise ConfigurationError('provision must identify exactly one team')
    # App ID prefixes may differ from Team IDs (Apple TN2415/TN3125).
    prefixes = profile.get('ApplicationIdentifierPrefix', [])
    if not isinstance(prefixes, list) or len(prefixes) != 1 or not isinstance(prefixes[0], str):
        raise ConfigurationError('provision must identify exactly one App ID prefix')
    if entitlements.get('com.apple.developer.team-identifier', teams[0]) != teams[0]:
        raise ConfigurationError('provision team entitlement is inconsistent')
    identifier = entitlements.get(APP_KEYS[platform])
    if identifier != prefixes[0] + '.' + topic:
        raise ConfigurationError('provision application identifier does not match the exact topic')
    expiry = profile.get('ExpirationDate')
    if not isinstance(expiry, datetime):
        raise ConfigurationError('provision expiration is missing')
    expiry = expiry.replace(tzinfo=timezone.utc) if expiry.tzinfo is None else expiry
    if expiry <= (now or datetime.now(timezone.utc)):
        raise ConfigurationError('provision has expired')
    return {'environment': environment, 'team': teams[0], 'application_identifier': identifier}


def prepare(info_path, entitlement_paths, platform, enabled, profile_path, topic):
    info = plistlib.loads(info_path.read_bytes())
    if not isinstance(info, dict):
        raise ConfigurationError('Info.plist must contain a dictionary')
    actual = info.get('CFBundleIdentifier')
    if actual not in (topic, '$(PRODUCT_BUNDLE_IDENTIFIER)', '${PRODUCT_BUNDLE_IDENTIFIER}'):
        raise ConfigurationError('build bundle identifier does not match the requested topic')
    contract = None
    if enabled:
        if profile_path is None:
            raise ConfigurationError('APNs enabled without an explicit real provisioning profile')
        contract = profile_contract(decode_profile(profile_path), platform, topic)
    # Validate every input before writing any generated file.
    entitlements = [(path, plistlib.loads(path.read_bytes())) for path in entitlement_paths]
    info['PPVPNApplePushEnabled'] = enabled
    if enabled:
        info['PPVPNAPNsEntitlementEnvironment'] = contract['environment']
    else:
        info.pop('PPVPNAPNsEntitlementEnvironment', None)
    info_path.write_bytes(plistlib.dumps(info))
    for path, value in entitlements:
        value.pop(ENV_KEYS[platform], None)
        if enabled:
            value[ENV_KEYS[platform]] = contract['environment']
        path.write_bytes(plistlib.dumps(value))
    return {'enabled': enabled, 'environment': contract['environment'] if contract else None,
            'topic': topic, 'stage': 'generated build configuration, not a signed-product check'}


def include_sources(app_root, platform, entry):
    app_root, entry = app_root.resolve(), entry.resolve()
    if entry.is_relative_to(app_root / "runner"):
        raise ConfigurationError("refusing to modify owned runner source templates")
    common = app_root / 'runner/apple/ApplePushState.swift'
    native = app_root / f'runner/{platform}/Runner/ApplePush.swift'
    text = entry.read_text()
    if text.count(MARKER) > 1 or text.count(END_MARKER) > 1:
        raise ConfigurationError('duplicate generated push source markers')
    if MARKER in text:
        before, _, rest = text.partition(MARKER)
        if END_MARKER not in rest:
            raise ConfigurationError('unterminated generated push source block')
        _, _, after = rest.partition(END_MARKER)
        text = before.rstrip() + '\n' + after.lstrip('\n')
    elif END_MARKER in text:
        raise ConfigurationError('unexpected generated push source end marker')
    copied = entry.parent / 'ApplePush.swift'
    native_text = native.read_text()
    if copied != native and copied.exists() and copied.read_text() != native_text:
        raise ConfigurationError('generated receiver differs from its source; refusing to remove it')
    common_text = common.read_text()
    entry.write_text(text.rstrip() + '\n\n' + MARKER + '\n' + common_text + '\n'
                     + native_text + '\n' + END_MARKER + '\n')
    # flutter create uses explicit build-file lists. Appending to the existing
    # entry includes both sources; remove the identical copied file to avoid
    # duplicate definitions in projects using synchronized groups instead.
    if copied != native and copied.exists():
        copied.unlink()


def read_signature(app):
    try:
        subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)],
                       capture_output=True, check=True)
        result = subprocess.run(['codesign', '-d', '--entitlements', ':-', str(app)],
                                capture_output=True, check=True)
        return plistlib.loads(result.stdout) if result.stdout else {}
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        raise ConfigurationError('bundle signature or signed entitlements could not be verified') from error


def check_contract(info, signed, profile, platform, expected_enabled, topic, distribution=False, app_store=False):
    if not isinstance(info, dict) or not isinstance(signed, dict):
        raise ConfigurationError('signed app metadata must contain dictionaries')
    if info.get('CFBundleIdentifier') != topic:
        raise ConfigurationError('final bundle identifier does not match the APNs topic')
    if info.get('PPVPNApplePushEnabled', False) is not expected_enabled:
        raise ConfigurationError('final push activation does not match the requested rollout')
    if not expected_enabled:
        if 'PPVPNAPNsEntitlementEnvironment' in info:
            raise ConfigurationError('disabled push build retains an environment stamp')
        return {'enabled': False, 'topic': topic, 'environment': None}
    contract = profile_contract(profile, platform, topic)
    environment = signed.get(ENV_KEYS[platform])
    if environment != contract['environment'] or environment != info.get('PPVPNAPNsEntitlementEnvironment'):
        raise ConfigurationError('final signed/profile/Info.plist APNs environments differ')
    if signed.get(APP_KEYS[platform]) != contract['application_identifier']:
        raise ConfigurationError('signed application identifier differs from its provision')
    if signed.get('com.apple.developer.team-identifier') != contract['team']:
        raise ConfigurationError('signed team differs from its provision')
    if (distribution or app_store) and (environment != 'production' or signed.get('get-task-allow') is True
                         or signed.get('com.apple.security.get-task-allow') is True
                         or profile.get('Entitlements', {}).get('get-task-allow') is True):
        raise ConfigurationError('distribution push requires production APNs and a distribution provision')
    if app_store and profile.get('ProvisionedDevices'):
        raise ConfigurationError('App Store export retains a device-limited provision')
    return {'enabled': True, 'topic': topic, 'environment': environment,
            'environment_source': 'matching final signed entitlements, embedded provision and app stamp'}


def verify_bundle(app, platform, enabled, topic, distribution=False, allow_unsigned_disabled=False, app_store=False):
    contents = app / 'Contents' if platform == 'macos' else app
    info = plistlib.loads((contents / 'Info.plist').read_bytes())
    if allow_unsigned_disabled:
        if enabled or distribution or app_store:
            raise ConfigurationError('unsigned inspection is allowed only for disabled-push diagnostic builds')
        signed = {}
    else:
        signed = read_signature(app)
    profile = {}
    if enabled:
        name = 'embedded.provisionprofile' if platform == 'macos' else 'embedded.mobileprovision'
        path = contents / name
        if not path.is_file():
            raise ConfigurationError('final bundle is missing its embedded provisioning profile')
        profile = decode_profile(path)
    result = check_contract(info, signed, profile, platform, enabled, topic, distribution, app_store)
    result['signature_verified'] = not allow_unsigned_disabled
    result['note'] = 'Configuration check only; does not prove backend registration or real push delivery.'
    return result


def verify_artifact(path, **kwargs):
    if path.suffix != '.ipa':
        return verify_bundle(path, **kwargs)
    if kwargs['platform'] != 'ios':
        raise ConfigurationError('IPA validation requires iOS')
    with zipfile.ZipFile(path) as archive, tempfile.TemporaryDirectory(prefix='ppvpn-ipa-check-') as work:
        names = set()
        total = 0
        for item in archive.infolist():
            member = PurePosixPath(item.filename)
            if member.is_absolute() or '..' in member.parts or item.filename in names:
                raise ConfigurationError('unsafe or duplicate IPA member')
            if stat.S_ISLNK(item.external_attr >> 16):
                raise ConfigurationError('IPA symlink requires explicit inspection; refusing unsafe extraction')
            names.add(item.filename)
            total += item.file_size
        if total + 512 * 1024 * 1024 > shutil.disk_usage(work).free:
            raise ConfigurationError('insufficient space for read-only IPA inspection')
        # ditto preserves the exported code/resources and Apple metadata.
        subprocess.run(['ditto', '-x', '-k', str(path), work], capture_output=True, check=True)
        apps = list((Path(work) / 'Payload').glob('*.app'))
        if len(apps) != 1:
            raise ConfigurationError('IPA must contain exactly one application')
        return verify_bundle(apps[0], **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    config = commands.add_parser('prepare')
    config.add_argument('--info', type=Path, required=True)
    config.add_argument('--entitlements', type=Path, action='append', required=True)
    config.add_argument('--profile', type=Path)
    sources = commands.add_parser('sources')
    sources.add_argument('--app-root', type=Path, required=True)
    sources.add_argument('--entry', type=Path, required=True)
    verify = commands.add_parser('verify')
    verify.add_argument('--artifact', type=Path, required=True)
    verify.add_argument('--distribution', action='store_true')
    verify.add_argument('--export-options', type=Path)
    verify.add_argument('--allow-unsigned-disabled', action='store_true')
    for command in (config, sources, verify):
        command.add_argument('--platform', choices=('ios', 'macos'), required=True)
    for command in (config, verify):
        command.add_argument('--enabled', choices=('0', '1'), default='0')
        command.add_argument('--topic', required=True)
    args = parser.parse_args()
    try:
        if args.command == 'sources':
            include_sources(args.app_root, args.platform, args.entry)
            print('Common and platform Apple push sources included; registration is not enabled.')
            return 0
        if args.command == 'prepare':
            result = prepare(args.info, args.entitlements, args.platform, args.enabled == '1',
                             args.profile, args.topic)
        else:
            distribution = args.distribution
            app_store = False
            if args.export_options:
                method = plistlib.loads(args.export_options.read_bytes()).get('method')
                if not method:
                    raise ConfigurationError('export options do not declare a method')
                app_store = method in ('app-store', 'app-store-connect')
                distribution |= method in ('app-store', 'app-store-connect', 'developer-id',
                                            'ad-hoc', 'release-testing', 'enterprise')
            result = verify_artifact(args.artifact, platform=args.platform, enabled=args.enabled == '1',
                topic=args.topic, distribution=distribution, app_store=app_store,
                allow_unsigned_disabled=args.allow_unsigned_disabled)
    except ConfigurationError as error:
        parser.exit(1, f'Apple push configuration rejected: {error}.\n')
    except (ValueError, OSError, subprocess.CalledProcessError, zipfile.BadZipFile):
        # No profile, token, certificate subject, device list or bearer in logs.
        parser.exit(1, 'Apple push build/configuration check failed; inspect private inputs locally.\n')
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
