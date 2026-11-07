#!/usr/bin/env python3
"""No-LLDB iPhone inventory/log snapshot. Never installs or controls an app/VPN.

The report deliberately cannot PASS forwarding, listener or single-runtime
acceptance from a process list or absence of errors. Pair it with the manual
checklist and exact signed-package provenance supplied by the build owner.
"""
import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path
import plistlib
import re
import subprocess
import urllib.parse
import zipfile

BUNDLE = 'com.peakpassvpn.ppvpn'
GROUP = 'group.com.peakpassvpn.ppvpn'
HOST_LOG_DIR = 'tunnel-state/logs'
HOST_LOG_PATHS = tuple(f'{HOST_LOG_DIR}/{name}' for name in ('tunnel-host.log', 'ppvpn-core.log'))


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def candidate_info(path):
    with zipfile.ZipFile(path) as archive:
        names = [n for n in archive.namelist() if re.fullmatch(r'Payload/[^/]+\.app/Info\.plist', n)]
        if len(names) != 1:
            raise ValueError('candidate must have exactly one app Info.plist')
        prefix = names[0].removesuffix('Info.plist')
        infos = []
        for name in [names[0], prefix + 'PlugIns/PacketTunnel.appex/Info.plist']:
            if archive.getinfo(name).file_size > 1024 * 1024:
                raise ValueError('oversized package metadata')
            infos.append(plistlib.loads(archive.read(name)))
        if infos[0].get('CFBundleIdentifier') != BUNDLE or infos[1].get('CFBundleIdentifier') != BUNDLE + '.tunnel':
            raise ValueError('candidate package identity mismatch')
        if infos[0].get('CFBundleVersion') != infos[1].get('CFBundleVersion'):
            raise ValueError('candidate app/extension build mismatch')
        return {'ipa_sha256': sha(path), 'build': str(infos[0]['CFBundleVersion']),
                'version': infos[0].get('CFBundleShortVersionString'),
                'apns_enabled': infos[0].get('PPVPNApplePushEnabled', False),
                'signature_verified_by_this_script': False,
                'note': 'Metadata only; use build-owner signed-profile/AppGroup/package verifier before installation.'}


def summarize(app_result, process_result, candidate=None):
    apps = [a for a in (app_result or {}).get('apps', []) if a.get('bundleIdentifier') == BUNDLE]
    app = apps[0] if len(apps) == 1 else None
    prefix = app.get('url', '') if app else ''
    processes = [p for p in (process_result or {}).get('runningProcesses', [])
                 if prefix and p.get('executable', '').startswith(prefix)]
    provider = [p for p in processes if '/PacketTunnel.appex/' in p.get('executable', '')]
    app_main = [p for p in processes if p not in provider]
    blockers = []
    if app_result is None or process_result is None:
        blockers.append('device inventory incomplete; no absence-of-process conclusion')
    if app is None:
        blockers.append('expected installed app not uniquely observed')
    if provider:
        blockers.append('existing provider process observed; confirm VPN Off and isolation before any active test/install')
    if candidate is None:
        blockers.append('final signed candidate/provenance not supplied')
    elif app and str(app.get('bundleVersion')) != candidate['build']:
        blockers.append('installed build is not the supplied candidate build')
    return {'installed_app': ({k: app.get(k) for k in ['bundleIdentifier', 'version', 'bundleVersion', 'builtByDeveloper']} if app else None),
            'app_process_pids': [p.get('processIdentifier') for p in app_main],
            'provider_process_pids': [p.get('processIdentifier') for p in provider],
            'candidate': candidate, 'preflight_blockers': blockers,
            'runtime_acceptance': 'NOT RUN', 'active_test_or_install_performed': False,
            'inference_limits': ['CoreDevice tunnelState is the USB developer transport, not NEVPN connection state.',
                'A provider PID does not prove forwarding, and absence does not prove no App listeners.',
                'Installed build number does not prove binary hash equality without installation provenance.',
                'Runtime owner/listener/production API evidence must come from the actual candidate.']}


def capture(out, name, arguments):
    path = out / (name + '.json')
    command = ['xcrun', 'devicectl', *arguments, '--timeout', '20', '--json-output', str(path)]
    try:
        result = subprocess.run(command, capture_output=True, timeout=30)
        (out / (name + '.command.log')).write_bytes(result.stdout + result.stderr)
    except subprocess.TimeoutExpired:
        return None
    if result.returncode != 0 or not path.exists():
        return None
    data = json.loads(path.read_text())
    return data.get('result') if data.get('info', {}).get('outcome') == 'success' else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--device', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--candidate-ipa', type=Path)
    parser.add_argument('--expected-origin', required=True)
    parser.add_argument('--copy-logs', action='store_true', help='Copy only allowlisted log files from device to local output; no auth/config/container copy')
    args = parser.parse_args()
    if not re.fullmatch(r'[0-9A-Fa-f-]{16,64}', args.device):
        parser.error('use the CoreDevice UUID/UDID from the read-only inventory')
    origin = urllib.parse.urlsplit(args.expected_origin)
    if origin.scheme != 'https' or not origin.hostname or origin.username or origin.password or origin.path not in ('', '/') or origin.query or origin.fragment:
        parser.error('expected origin must be the formal HTTPS origin without credentials/path/query')
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False, mode=0o700)
    candidate = candidate_info(args.candidate_ipa) if args.candidate_ipa else None
    apps = capture(out, 'apps', ['device', 'info', 'apps', '--device', args.device])
    processes = capture(out, 'processes', ['device', 'info', 'processes', '--device', args.device])
    lock = capture(out, 'lock', ['device', 'info', 'lockState', '--device', args.device])
    report = summarize(apps, processes, candidate)
    report['captured_at_utc'] = dt.datetime.now(dt.timezone.utc).isoformat()
    report['expected_formal_origin'] = args.expected_origin.rstrip('/')
    report['running_api_origin_verified'] = False
    report['lock_state'] = lock
    report['log_listings'] = {}
    for label, subdir in [('app-logs', 'logs'), ('host-logs', HOST_LOG_DIR)]:
        files = capture(out, label, ['device', 'info', 'files', '--device', args.device,
            '--domain-type', 'appGroupDataContainer', '--domain-identifier', GROUP,
            '--subdirectory', subdir, '--no-recurse'])
        report['log_listings'][label] = 'available' if files is not None else 'unavailable; not proof of no errors'
    report['copied_logs'] = []
    if args.copy_logs:
        today = dt.datetime.now(dt.timezone.utc).date()
        paths = list(HOST_LOG_PATHS)
        for day in [today, today - dt.timedelta(days=1)]:
            paths += [f'logs/ppvpn-client.{day}.log', f'logs/ppvpn-core.{day}.log']
        logs = out / 'logs'; logs.mkdir(mode=0o700)
        for number, source in enumerate(paths):
            destination = logs / Path(source).name
            result = capture(out, 'copy-log-' + str(number), ['device', 'copy', 'from', '--device', args.device,
                '--domain-type', 'appGroupDataContainer', '--domain-identifier', GROUP,
                '--source', source, '--destination', str(destination)])
            if result is not None and destination.is_file():
                text = destination.read_text(errors='replace')
                report['copied_logs'].append({'name': destination.name, 'sha256': sha(destination), 'bytes': destination.stat().st_size,
                    'review_markers': {name: len(re.findall(pattern, text, re.I)) for name, pattern in {
                        'panic_fatal': r'\bpanic\b|\bfatal\b', 'capture_uncertain': r'capture.*(?:dirty|failed)|settings.*timeout',
                        'app_group_failure': r'no App Group container', 'runtime_conflict': r'already running|another.*owner|socket.*in use'}.items()}})
    (out / 'summary.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({'output': str(out / 'summary.json'), 'blockers': report['preflight_blockers'],
                      'runtime_acceptance': 'NOT RUN', 'device_mutations': False}, ensure_ascii=False))


if __name__ == '__main__':
    main()
