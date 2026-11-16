#!/usr/bin/env python3
"""Review by default. Execution is confined to a fresh hosted arm64 Mac VM.
Builds/tests only the synthetic AppKit/XCTest pair; never installs/runs359.
"""
import argparse
import datetime
import hashlib
import json
import os
import pathlib
import platform
import signal
import shutil
import subprocess
import sys
import time

from watchdog import cleanup


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--execute', action='store_true')
    p.add_argument('--approved-manifest-sha256')
    p.add_argument('--out', type=pathlib.Path)
    a = p.parse_args()
    source = pathlib.Path(__file__).resolve().parent
    manifest_path = source / 'INPUT-MANIFEST.json'
    manifest = json.loads(manifest_path.read_text())
    for name, expected in manifest.items():
        path = (source / name).resolve(strict=True)
        if not path.is_relative_to(source) or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            p.error('Probe input changed: ' + name)
    digest = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
    if not a.execute:
        print(json.dumps({'status': 'REVIEW_ONLY_NO_COMPILATION_OR_UI', 'input_manifest_sha256': digest,
                          'App359_installed_or_launched': False, 'host_settings_changed': False}))
        return 0
    if a.approved_manifest_sha256 != digest:
        p.error('Exact reviewed manifest SHA is required for execution')
    env = os.environ
    if platform.system() != 'Darwin' or platform.machine() != 'arm64' or os.getuid() == 0 or env.get('GITHUB_ACTIONS') != 'true' or env.get('RUNNER_ENVIRONMENT') != 'github-hosted' or not env.get('GITHUB_RUN_ID', '').isdigit():
        p.error('Fresh non-root hosted arm64 macOS runner required; never execute on the user Mac')
    temp = pathlib.Path(env.get('RUNNER_TEMP', '')).resolve(strict=True)
    if not a.out or not a.out.resolve().is_relative_to(temp) or a.out.exists():
        p.error('New exclusive output directory under RUNNER_TEMP required')
    forbidden = ['/Applications/PPVPN.app', '/Library/PrivilegedHelperTools/com.peakpassvpn.ppvpn.service.bundle',
                 '/Library/LaunchDaemons/com.peakpassvpn.ppvpn.service.plist',
                 '/Library/LaunchAgents/com.peakpassvpn.ppvpn.user-runtime.plist']
    if any(pathlib.Path(path).exists() for path in forbidden):
        p.error('Existing PPVPN installation refused; not a fresh probe environment')
    for tool in ['xcodebuild', 'xcrun']:
        if not shutil.which(tool):
            p.error('Pre-stage the managed tool before runtime: ' + tool)
    os.umask(0o077)
    a.out.mkdir(mode=0o700)
    out = a.out.resolve()
    work = out / 'source'
    work.mkdir(mode=0o700)
    for name in manifest:
        target = work / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source / name, target)
    derived = out / 'derived'
    report = {'status': 'NOT_ACCEPTED', 'input_manifest_sha256': digest,
              'observed_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'scope': 'synthetic WindowServer/XCUI only; no signed359 runtime acceptance',
              'App359_installed_or_launched': False, 'settings_or_services_changed': False}
    wd = None
    wd_deadline = None
    log_handles = []

    def run(command, name, timeout):
        log = (out / (name + '.log')).open('w')
        log_handles.append(log)
        child = subprocess.Popen(command, cwd=work, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            return child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=10)
            return 124

    try:
        preflight = run(['xcrun', 'swift', str(work / 'WindowServerPreflight.swift')], 'WindowServer-preflight', 45)
        report['WindowServer_preflight_exit'] = preflight
        if preflight == 77:
            raise RuntimeError('WINDOWSERVER_OR_ACTIVE_CONSOLE_NOT_AVAILABLE')
        if preflight:
            raise RuntimeError('WINDOWSERVER_PREFLIGHT_COMPILATION_OR_EXECUTION_FAILED_NOT_GUI_EVIDENCE')
        base = ['xcodebuild', '-project', str(work / 'Auto359SyntheticCapability.xcodeproj'),
                '-scheme', 'SyntheticWindowRoundTrip', '-configuration', 'Debug',
                '-destination', 'platform=macOS,arch=arm64', '-derivedDataPath', str(derived),
                '-disableAutomaticPackageResolution', '-skipPackageUpdates',
                'CODE_SIGN_STYLE=Manual', 'CODE_SIGN_IDENTITY=-', 'DEVELOPMENT_TEAM=']
        if run(base + ['build-for-testing'], 'build-for-testing', 300):
            raise RuntimeError('SYNTHETIC_BUILD_FAILED')
        registry = out / 'WATCHDOG-REGISTRY.json'
        wd_deadline = time.time() + 210
        registry.write_text(json.dumps({'derived': str(derived), 'deadline_unix': wd_deadline}) + '\n')
        wd = subprocess.Popen([sys.executable, str(work / 'watchdog.py'), str(registry)], start_new_session=True)
        result = out / 'result.xcresult'
        test = run(base + ['test-without-building', '-parallel-testing-enabled', 'NO',
                          '-only-testing:Auto359CapabilityUITests/WindowRoundTripTests/testSyntheticWindowAndTwoRealClicks',
                          '-resultBundlePath', str(result)], 'test-without-building', 150)
        report['XCTest_exit'] = test
        if test:
            raise RuntimeError('XCTEST_UI_ROUNDTRIP_FAILED_OR_TIMED_OUT')
        summary = subprocess.run(['xcrun', 'xcresulttool', 'get', 'test-results', 'summary', '--path', str(result)],
                                 capture_output=True, text=True, timeout=30)
        (out / 'xcresult-summary.json').write_text(summary.stdout)
        (out / 'xcresult-summary.stderr').write_text(summary.stderr)
        if summary.returncode:
            raise RuntimeError('XCRESULT_SUMMARY_UNAVAILABLE_NOT_ACCEPTED')
        value = json.loads(summary.stdout)
        if value.get('result') != 'Passed' or value.get('totalTestCount') != 1 or value.get('passedTests') != 1 or value.get('failedTests') != 0 or value.get('skippedTests') != 0:
            raise RuntimeError('ONE_REAL_UI_TEST_PASS_NOT_PROVEN_OR_SUMMARY_SCHEMA_UNKNOWN')
        report['status'] = 'SYNTHETIC_XCUI_CAPABILITY_PASS_NOT_APP359_ACCEPTANCE'
    except Exception as error:
        report['failure'] = str(error)
    finally:
        if derived.exists():
            try:
                report['owned_cleanup'] = cleanup(derived)
                if report['owned_cleanup']['remaining']:
                    report['status'] = 'CLEANUP_FAILED_NOT_ACCEPTED'
            except Exception as error:
                report['status'] = 'CLEANUP_UNKNOWN_NOT_ACCEPTED'
                report['cleanup_error'] = str(error)
        if (out / 'WATCHDOG-EXPIRED.json').exists() or (wd_deadline is not None and time.time() >= wd_deadline):
            report['status'] = 'WATCHDOG_EXPIRED_NOT_ACCEPTED'
        cleanup_known = report.get('owned_cleanup', {}).get('remaining') == [] or not derived.exists()
        if cleanup_known:
            (out / 'COMPLETE.json').write_text(json.dumps({'controller_finished': True}) + '\n')
        else:
            # Keep the independent bounded watchdog armed if cleanup is not
            # proven; this run is already not accepted and must not claim PASS.
            report['watchdog_still_armed_until_deadline'] = wd is not None
        if wd and cleanup_known:
            wd.wait(timeout=5)
        for log in log_handles:
            log.close()
        (out / 'RESULT.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))
    return 0 if report['status'] == 'SYNTHETIC_XCUI_CAPABILITY_PASS_NOT_APP359_ACCEPTANCE' else 1


if __name__ == '__main__':
    raise SystemExit(main())
