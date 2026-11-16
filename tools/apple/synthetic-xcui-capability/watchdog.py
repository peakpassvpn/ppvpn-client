#!/usr/bin/env python3
"""Bounded nonprivileged cleanup of this synthetic probe's exact derived images."""
import json
import os
import pathlib
import signal
import subprocess
import sys
import time

NAMES = {'Auto359Capability', 'Auto359CapabilityUITests-Runner'}


def owned(derived):
    result = subprocess.run(['/bin/ps', '-ww', '-axo', 'pid=,uid=,comm='], capture_output=True, text=True, timeout=10, check=True)
    rows = []
    for line in result.stdout.splitlines():
        words = line.split(None, 2)
        if len(words) != 3 or not words[0].isdigit() or not words[1].isdigit():
            continue
        path = pathlib.Path(words[2])
        if int(words[1]) == os.getuid() and path.name in NAMES and path.is_relative_to(derived):
            rows.append({'pid': int(words[0]), 'path': str(path)})
    return rows


def cleanup(derived):
    signaled = []
    for row in owned(derived):
        try:
            os.kill(row['pid'], signal.SIGTERM)
            signaled.append(row)
        except ProcessLookupError:
            pass
    for _ in range(30):
        if not owned(derived):
            break
        time.sleep(0.1)
    # Re-observe exact uid/path before any final signal; never killall, pkill,
    # an OS testmanager service or a different application.
    for row in owned(derived):
        try:
            os.kill(row['pid'], signal.SIGKILL)
            signaled.append(row)
        except ProcessLookupError:
            pass
    return {'signaled': signaled, 'remaining': owned(derived)}


def main():
    registry = pathlib.Path(sys.argv[1]).resolve(strict=True)
    value = json.loads(registry.read_text())
    temp = pathlib.Path(os.environ['RUNNER_TEMP']).resolve(strict=True)
    derived = pathlib.Path(value['derived']).resolve(strict=True)
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted' or os.getuid() == 0 or not derived.is_relative_to(temp):
        raise SystemExit('MANAGED_SYNTHETIC_SCOPE_REQUIRED')
    complete = registry.parent / 'COMPLETE.json'
    while time.time() < value['deadline_unix']:
        if complete.exists():
            return
        time.sleep(1)
    result = cleanup(derived)
    (registry.parent / 'WATCHDOG-EXPIRED.json').write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
