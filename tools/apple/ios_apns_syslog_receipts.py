#!/usr/bin/env python3
"""Passive physical-iPhone receipt text capture for idevicesyslog1.4.0.

Never starts/installs an App or changes a VPN. Binds a root-observed current
Runner PID via the real device process table. No synthetic unified NDJSON:
this text format omits subsystem/category, so matching is NOT OS delivery proof.
"""
import argparse
import collections
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import time

from apns_trace_receipts import parse_message, parse_settings_message

HEADER = re.compile(
    r'(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) {1,2}'
    r'(?:[1-9]|[12][0-9]|3[01]) '
    r'(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]\.[0-9]{6} '
    r'(?P<process>[A-Za-z_][A-Za-z_0-9.-]*)(?:\((?P<image>[^)\r\n]{1,100})\))?'
    r'\[(?P<pid>[1-9][0-9]*)\] <(?P<level>[A-Za-z]+)>: (?P<message>[^\r\n]*)')


def parse_syslog_line(line, expected_pid):
    if not isinstance(line, str) or len(line) > 65536:
        return None
    header = HEADER.fullmatch(line)
    if not header or header['process'] != 'Runner' or int(header['pid']) != expected_pid:
        return None
    # Production release trace is emitted from Runner, not Flutter/other images.
    if header['image'] not in (None, 'Runner') or header['level'] != 'Info':
        return None
    receipt = parse_message(header['message']) or parse_settings_message(header['message'])
    if receipt is None:
        return None
    return {'observation_kind': 'PROCESS_BOUND_FIXED_TRACE_TEXT',
            'subsystem_category_verified': False, 'os_delivery_inferred': False,
            'receipt': receipt}


def pidlist_matches(raw, expected_pid):
    # PID/name table has actually been observed as "PID Runner" on this
    # phone; no raw table, process arguments or other process names retained.
    matches = []
    for line in raw.decode('utf-8', errors='replace').splitlines():
        row = re.fullmatch(r'([1-9][0-9]*)\s+(.+)', line.strip())
        if row and int(row[1]) == expected_pid:
            matches.append(row[2])
    return matches == ['Runner']


def check_pid(tool, udid, pid):
    try:
        result = subprocess.run([tool, '-u', udid, 'pidlist'], capture_output=True, timeout=15)
        return result.returncode == 0 and pidlist_matches(result.stdout, pid)
    except (OSError, subprocess.TimeoutExpired):
        return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--device-udid', required=True)
    parser.add_argument('--pid', required=True, type=int, help='actual installed candidate Runner PID observed by root; never old build PID')
    parser.add_argument('--duration', type=int, default=180, help='bounded passive seconds,15..1800')
    parser.add_argument('--output', type=Path, required=True, help='new private NDJSON text-observation file; not unified-log NDJSON')
    args = parser.parse_args()
    if not re.fullmatch(r'(?:[0-9A-Fa-f]{8}-[0-9A-Fa-f]{16}|[0-9A-Fa-f]{40})', args.device_udid):
        parser.error('expected hardware UDID, not CoreDevice UUID')
    if args.pid <= 0 or not 15 <= args.duration <= 1800:
        parser.error('positive observed PID and bounded duration15..1800 required')
    tool = shutil.which('idevicesyslog')
    if tool is None:
        parser.exit(2, 'IDEVICESYSLOG_NOT_INSTALLED\n')
    if not check_pid(tool, args.device_udid, args.pid):
        parser.exit(2, 'CURRENT_DEVICE_PID_NAME_BINDING_NOT_VERIFIED\n')
    fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    summary_path = args.output.with_name(args.output.name + '.summary.json')
    try:
        summary_fd = os.open(summary_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except OSError:
        os.close(fd)
        raise
    summary = {'capture_source': 'idevicesyslog_text', 'expected_process': 'Runner',
               'expected_pid': args.pid, 'initial_pid_name_binding_verified': True,
               'subsystem_category_available': False, 'runtime_acceptance': 'NOT INFERRED',
               'os_delivery_inferred': False, 'raw_log_saved': False, 'app_or_vpn_action': False,
               'limits': ['Root must independently tie supplied PID to exact installed signed candidate.',
                          'Text relay omits namespace metadata and may contain multiline messages; matching text is not authenticated native callback or OS delivery proof.',
                          'No automatic PID retarget after restart/cold launch; stop and bind newly observed candidate PID.',
                          'Zero rows/missing early callbacks are NOT OBSERVED, not no errors or no OS delivery.']}
    counts = collections.Counter()
    discarded = observed = 0
    child = None
    selector = selectors.DefaultSelector()
    started = time.monotonic()
    buffer = b''
    dropping_oversized = False
    def stop_capture(_signum, _frame):
        raise KeyboardInterrupt
    previous_terminate = signal.signal(signal.SIGTERM, stop_capture)
    try:
        with os.fdopen(fd, 'w') as output:
            child = subprocess.Popen([tool, '-u', args.device_udid, '--no-colors', '--exit',
                                      '-p', str(args.pid), '-m', 'apns'],
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                     stdin=subprocess.DEVNULL)
            selector.register(child.stdout, selectors.EVENT_READ)
            os.set_blocking(child.stdout.fileno(), False)
            next_identity_check = time.monotonic() + 3
            summary['stop_reason'] = 'DURATION_REACHED'
            while time.monotonic() - started < args.duration:
                if child.poll() is not None:
                    summary['stop_reason'] = 'DEVICE_RELAY_EXITED_OR_DISCONNECTED'
                    summary['relay_exit_code'] = child.returncode
                    break
                if time.monotonic() >= next_identity_check:
                    if not check_pid(tool, args.device_udid, args.pid):
                        summary['stop_reason'] = 'PID_NAME_BINDING_LOST_OR_UNAVAILABLE'
                        break
                    next_identity_check = time.monotonic() + 3
                for key, _ in selector.select(timeout=0.25):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        continue
                    buffer += chunk
                    while b'\n' in buffer:
                        line, buffer = buffer.split(b'\n', 1)
                        if dropping_oversized or len(line) > 65536:
                            dropping_oversized = False
                            discarded += 1
                            continue
                        try:
                            observation = parse_syslog_line(line.decode('utf-8'), args.pid)
                        except UnicodeDecodeError:
                            observation = None
                        if observation is None:
                            discarded += 1
                            continue
                        output.write(json.dumps(observation, separators=(',', ':')) + '\n')
                        output.flush()
                        observed += 1
                        counts[observation['receipt']['stage']] += 1
                    if len(buffer) > 65536:
                        buffer = b''
                        dropping_oversized = True
    except KeyboardInterrupt:
        summary['stop_reason'] = 'OPERATOR_INTERRUPTED_PASSIVE_CAPTURE'
    except (OSError, subprocess.SubprocessError):
        summary['stop_reason'] = 'RELAY_CAPTURE_UNAVAILABLE_NO_RAW_DETAILS_EXPORTED'
    finally:
        signal.signal(signal.SIGTERM, previous_terminate)
        selector.close()
        if child is not None:
            if child.poll() is None:
                child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill(); child.wait(timeout=3)
            child.stdout.close()
        summary.update(elapsed_seconds=round(time.monotonic() - started, 1),
                       text_receipt_count=observed, discarded_lines=discarded, stage_counts=dict(counts))
        with os.fdopen(summary_fd, 'w') as output:
            json.dump(summary, output, indent=2); output.write('\n')
    print(json.dumps({'text_receipt_count': observed, 'stop_reason': summary['stop_reason'],
                      'runtime_acceptance': 'NOT INFERRED', 'os_delivery_inferred': False}))


if __name__ == '__main__':
    main()
