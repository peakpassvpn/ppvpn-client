#!/usr/bin/env python3
"""Bounded real OS-token probe for a fresh GitHub-hosted Mac GUI job only.

Runs an already reviewed signed candidate through LaunchServices and observes
native receipts. No login, provider key, backend binding, send or system config.
Never execute on the user's working Mac. Workflow/signing remain builder-owned.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import selectors
import signal
import subprocess
import sys
import time

from apns_trace_receipts import CATEGORY, SUBSYSTEM, parse

PROBE_KIND = 'native_apns_os_registration_capability_only'


def source_hashes(manifest):
    expected = {'apps/flutter/runner/apple/ApplePushState.swift',
                'apps/flutter/runner/macos/Runner/ApplePush.swift'}
    values = manifest.get('source_sha256')
    if not isinstance(values, dict) or set(values) != expected:
        raise Refused('INVALID_DIAGNOSTIC_SOURCE_HASHES')
    if any(not isinstance(value, str) or len(value) != 64 or
           any(c not in '0123456789abcdef' for c in value) for value in values.values()):
        raise Refused('INVALID_DIAGNOSTIC_SOURCE_HASHES')
    return values


class Refused(RuntimeError):
    pass


def require_hosted_mac(env, platform):
    if platform != 'darwin' or env.get('GITHUB_ACTIONS') != 'true' or env.get('RUNNER_ENVIRONMENT') != 'github-hosted':
        raise Refused('NOT_A_GITHUB_HOSTED_MAC_JOB')
    if not env.get('RUNNER_TEMP') or not env.get('GITHUB_RUN_ID'):
        raise Refused('MISSING_HOSTED_JOB_CONTEXT')


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def run(command, timeout=30):
    try:
        return subprocess.run(command, capture_output=True, timeout=timeout)
    except (OSError, subprocess.TimeoutExpired):
        raise Refused('PREFLIGHT_COMMAND_UNAVAILABLE') from None


def processes():
    result = run(['/bin/ps', '-axww', '-o', 'pid=', '-o', 'comm='])
    if result.returncode:
        raise Refused('PROCESS_INVENTORY_UNAVAILABLE')
    rows = []
    for line in result.stdout.decode(errors='replace').splitlines():
        parts = line.strip().split(None, 1)
        if len(parts) == 2 and parts[0].isdigit():
            rows.append((int(parts[0]), parts[1]))
    return rows


def outcome(receipts):
    # Callers preserve source PID privately while filtering the exact image
    # path. A request from another launch cannot prove this callback's chain.
    requests = set()
    for pid, receipt in receipts:
        if receipt['stage'] == 'register_requested':
            requests.add(pid)
        elif pid in requests and receipt['stage'] == 'registration_failed':
            return 'OS_REGISTRATION_ERROR_OBSERVED'
        elif pid in requests and receipt['stage'] == 'token_received' and receipt['accepted']:
            return 'OS_TOKEN_CALLBACK_OBSERVED'
    return None


def probe(args, report):
    require_hosted_mac(os.environ, sys.platform)
    if os.getuid() == 0:
        raise Refused('REFUSE_ROOT_GUI_LAUNCH')
    temp = Path(os.environ['RUNNER_TEMP']).resolve(strict=True)
    app = args.app.resolve(strict=True)
    if app.suffix != '.app' or not app.is_dir() or not app.is_relative_to(temp):
        raise Refused('APP_MUST_BE_STAGED_IN_OWNED_RUNNER_TEMP')
    if '/Applications/' in str(app):
        raise Refused('REFUSE_AGENT_INSTALL_LOCATION_FOR_TOKEN_PROBE')
    if app.stat().st_uid != os.getuid():
        raise Refused('APP_NOT_OWNED_BY_JOB_USER')
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    if info.get('CFBundleIdentifier') != 'com.peakpassvpn.ppvpn.desktop' or info.get('PPVPNApplePushEnabled') is not True:
        raise Refused('WRONG_IDENTITY_OR_PUSH_NOT_EXPLICITLY_ENABLED')
    name = info.get('CFBundleExecutable')
    if not isinstance(name, str) or not name or '/' in name:
        raise Refused('INVALID_EXECUTABLE_METADATA')
    executable = (app / 'Contents/MacOS' / name).resolve(strict=True)
    if not executable.is_relative_to(app):
        raise Refused('EXECUTABLE_ESCAPES_REVIEWED_BUNDLE')
    profile = app / 'Contents/embedded.provisionprofile'
    verifier = args.verifier.resolve(strict=True)
    for path, expected, error in [(executable, args.executable_sha256, 'EXECUTABLE_HASH_MISMATCH'),
                                   (profile, args.profile_sha256, 'PROFILE_HASH_MISMATCH'),
                                   (verifier, args.verifier_sha256, 'VERIFIER_HASH_MISMATCH')]:
        if digest(path) != expected:
            raise Refused(error)
    build = info.get('CFBundleVersion')
    if not isinstance(build, str) or not build.isascii() or not build.isdigit():
        raise Refused('INVALID_BUILD_NUMBER')
    report.update(executable_sha256=args.executable_sha256, profile_sha256=args.profile_sha256,
                  verifier_sha256=args.verifier_sha256, build=build,
                  legacy_agent_files_preserved=(app / 'Contents/Library/LoginItems/PPVPN Agent.app').is_dir())
    if info.get('PPVPNAPNsCapabilityProbe') is True:
        manifest_path = app / 'Contents/Resources/APNsCapabilityProbe.json'
        manifest = json.loads(manifest_path.read_text())
        if manifest.get('kind') != PROBE_KIND:
            raise Refused('INVALID_DIAGNOSTIC_PROBE_MANIFEST')
        report['candidate_kind'] = PROBE_KIND
        report['production_receiver_source_sha256'] = source_hashes(manifest)
        report['legacy_agent_status'] = 'Diagnostic host does not include Agent; ordinary packaged App/Agent unchanged'
    elif not report['legacy_agent_files_preserved']:
        raise Refused('PACKAGED_LEGACY_AGENT_MISSING')
    else:
        report['candidate_kind'] = 'full_packaged_app_OS_registration_only'
    details = run(['/usr/bin/codesign', '-dv', '--verbose=4', str(app)])
    if details.returncode or b'Authority=Developer ID Application:' not in details.stderr:
        raise Refused('NOT_DEVELOPER_ID_APPLICATION_SIGNED')
    # Existing builder verifier checks signature/profile/certificate membership,
    # exact topic and production environment. No raw diagnostics are exported.
    checked = run([sys.executable, str(verifier), 'verify', '--platform', 'macos',
                   '--artifact', str(app), '--enabled', '1', '--topic',
                   'com.peakpassvpn.ppvpn.desktop', '--distribution'], timeout=60)
    if checked.returncode:
        raise Refused('BUILDER_SIGNED_BUNDLE_VERIFIER_REJECTED')
    console = run(['/usr/bin/stat', '-f', '%u', '/dev/console'])
    gui = run(['/bin/launchctl', 'print', 'gui/' + str(os.getuid())])
    if console.returncode or console.stdout.strip() != str(os.getuid()).encode() or gui.returncode:
        raise Refused('NO_MATCHING_CONSOLE_GUI_SESSION')
    # Refuse reused PPVPN state. Do not read, copy or clear credential files
    # or Keychain contents; fresh VM plus no App login is the boundary here.
    home = Path.home()
    if any(path.exists() for path in [home / 'Library/Application Support/PPVPN', home / 'Library/Logs/PPVPN']):
        raise Refused('PREEXISTING_PPVPN_USER_DATA')
    before = processes()
    if any(Path(path).name in ('PPVPN', 'PPVPN Agent', 'ppvpn-service') or path == str(executable) for _, path in before):
        raise Refused('PREEXISTING_PPVPN_PROCESS')
    agent = run(['/bin/launchctl', 'print', 'gui/' + str(os.getuid()) + '/com.peakpassvpn.ppvpn.desktop.agent'])
    if agent.returncode == 0:
        raise Refused('PREEXISTING_PPVPN_AGENT_JOB')
    report['preflight'] = 'PASSED_FOR_TOKEN_PROBE_ONLY'
    predicate = 'subsystem == "' + SUBSYSTEM + '" AND category == "' + CATEGORY + '"'
    stream = subprocess.Popen(['/usr/bin/log', 'stream', '--style', 'ndjson', '--level', 'info', '--predicate', predicate],
                              stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL)
    selector = selectors.DefaultSelector()
    selector.register(stream.stdout, selectors.EVENT_READ)
    os.set_blocking(stream.stdout.fileno(), False)
    receipts = []
    baseline_pids = {pid for pid, _ in before}
    buffer = b''
    launched = False
    started = time.monotonic()
    try:
        # Check capture-process survival before starting the candidate. Sleep
        # is confined to this remote one-shot job, not this preparation chat.
        time.sleep(1)
        if stream.poll() is not None:
            report['log_stream_exit_code'] = stream.returncode
            raise Refused('UNIFIED_LOG_STREAM_UNAVAILABLE')
        opened = run(['/usr/bin/open', '-n', str(app)], timeout=20)
        if opened.returncode:
            raise Refused('LAUNCHSERVICES_REJECTED_CANDIDATE')
        launched = True
        report['launch_requested'] = True
        report['app_launched'] = any(path == str(executable) and pid not in baseline_pids for pid, path in processes())
        while time.monotonic() - started < args.timeout:
            if stream.poll() is not None:
                report['result'] = 'UNIFIED_LOG_STREAM_EXITED'
                report['log_stream_exit_code'] = stream.returncode
                break
            for key, _ in selector.select(timeout=0.5):
                chunk = os.read(key.fileobj.fileno(), 65536)
                if not chunk:
                    continue
                buffer += chunk
                while b'\n' in buffer:
                    line, buffer = buffer.split(b'\n', 1)
                    if len(line) > 65536:
                        continue
                    try:
                        event = json.loads(line)
                        receipt = parse(event, set(), str(executable))
                    except (ValueError, UnicodeDecodeError):
                        continue
                    if receipt is not None:
                        pid = event.get('processID', event.get('processIdentifier'))
                        if pid not in baseline_pids:
                            report['app_launched'] = True
                            receipts.append((pid, receipt))
                            if len(receipts) >= 256:
                                raise Refused('RECEIPT_LIMIT_REACHED')
                if len(buffer) > 65536:
                    buffer = b''
            result = outcome(receipts)
            if result:
                report['result'] = result
                break
        else:
            report['result'] = 'TIMED_OUT_NO_VERIFIED_OS_TOKEN_CHAIN'
    finally:
        report['elapsed_seconds'] = round(time.monotonic() - started, 1)
        # Stop only the capture child and newly launched exact-image candidate
        # in this fresh VM. Never pkill by name or touch an Agent/VPN service.
        selector.close()
        if stream.poll() is None:
            stream.terminate()
        try:
            stream.wait(timeout=5)
        except subprocess.TimeoutExpired:
            stream.kill(); stream.wait(timeout=5)
        stream.stdout.close()
        stopped = 0
        try:
            if launched:
                for pid, path in processes():
                    if path == str(executable) and pid not in baseline_pids:
                        try:
                            os.kill(pid, signal.SIGTERM)
                            stopped += 1
                        except ProcessLookupError:
                            pass
        except Refused:
            report['owned_candidate_cleanup_inventory_failed'] = True
        report['owned_candidate_termination_requests'] = stopped
        report['receipts'] = [receipt for _, receipt in receipts]
        report['stage_counts'] = {stage: sum(r['stage'] == stage for _, r in receipts)
                                  for stage in sorted({r['stage'] for _, r in receipts})}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', type=Path, required=True)
    parser.add_argument('--executable-sha256', required=True)
    parser.add_argument('--profile-sha256', required=True)
    parser.add_argument('--verifier', type=Path, required=True, help='builder-owned push_config.py')
    parser.add_argument('--verifier-sha256', required=True)
    parser.add_argument('--timeout', type=int, default=120)
    parser.add_argument('--output', type=Path, required=True, help='new private JSON result path')
    args = parser.parse_args()
    if not 15 <= args.timeout <= 180:
        parser.error('timeout must be15..180 seconds')
    for value in (args.executable_sha256, args.profile_sha256, args.verifier_sha256):
        if len(value) != 64 or any(c not in '0123456789abcdef' for c in value):
            parser.error('expected SHA256 must be64 lowercase hex characters')
    # Refuse the working Mac before even creating an output file.
    try:
        require_hosted_mac(os.environ, sys.platform)
    except Refused as error:
        parser.exit(2, str(error) + '\n')
    fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    report = {'result': 'NOT_EXECUTED', 'launch_requested': False, 'app_launched': False, 'backend_login': 'NOT RUN',
              'provider_or_worker_send': 'NOT RUN', 'real_delivery_acceptance': 'NOT RUN',
              'notification_permission': 'NOT REQUESTED_BY_HELPER', 'raw_logs_saved': False,
              'scope': 'OS callback feasibility only; timeout is not proof that all VMs lack APNs'}
    with os.fdopen(fd, 'w') as output:
        try:
            probe(args, report)
        except Refused as error:
            report['result'] = str(error)
        except (OSError, ValueError, KeyError, subprocess.SubprocessError):
            report['result'] = 'PROBE_INFRASTRUCTURE_ERROR_NO_RAW_DETAILS_EXPORTED'
        json.dump(report, output, indent=2)
        output.write('\n')
    print(json.dumps({'result': report['result'], 'real_delivery_acceptance': 'NOT RUN'}))
    return 0 if report['result'] == 'OS_TOKEN_CALLBACK_OBSERVED' else 2


if __name__ == '__main__':
    sys.exit(main())
