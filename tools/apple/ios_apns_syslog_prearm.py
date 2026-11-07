#!/usr/bin/env python3
"""Passive prelaunch iPhone trace-text capture; persist only after exact PID binding.

Raw relay lines stay in memory. Before root supplies the actual candidate PID,
only fixed parsed observations are buffered (max128), never raw log strings.
No App launch/install, device/VPN control or inferred OS delivery.
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
import stat
import subprocess
import time

from ios_apns_syslog_receipts import HEADER, check_pid, parse_syslog_line


def read_binding(path):
    if not path.exists():
        return None
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        raise ValueError('UNSAFE_PID_BINDING_FILE')
    with path.open('rb') as stream:
        value = stream.read(65)
    if not re.fullmatch(rb'[1-9][0-9]{0,9}\n?', value):
        raise ValueError('INVALID_PID_BINDING_FILE')
    return int(value.strip())


def pending_observation(line):
    if len(line) > 65536:
        return None
    header = HEADER.fullmatch(line)
    if header is None:
        return None
    pid = int(header['pid'])
    observation = parse_syslog_line(line, pid)
    return (pid, observation) if observation is not None else None


def select_bound(pending, pid):
    return [observation for source_pid, observation in pending if source_pid == pid]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--device-udid', required=True)
    parser.add_argument('--pid-file', type=Path, required=True, help='MUST NOT EXIST until root writes newly observed PID with mode0600')
    parser.add_argument('--duration', type=int, default=600)
    parser.add_argument('--bind-timeout', type=int, default=300)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r'(?:[0-9A-Fa-f]{8}-[0-9A-Fa-f]{16}|[0-9A-Fa-f]{40})', args.device_udid):
        parser.error('hardware device UDID required')
    if not 15 <= args.bind_timeout <= args.duration <= 1800:
        parser.error('require15 <= bind-timeout <= duration <=1800 seconds')
    if args.pid_file.exists() or args.pid_file.is_symlink():
        parser.exit(2, 'PID_BINDING_FILE_MUST_BE_NEW_AND_ABSENT\n')
    tool = shutil.which('idevicesyslog')
    if tool is None:
        parser.exit(2, 'IDEVICESYSLOG_NOT_INSTALLED\n')
    fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        summary_fd = os.open(str(args.output) + '.summary.json', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except OSError:
        os.close(fd)
        raise
    summary = {'capture_source': 'idevicesyslog_text_prearmed', 'runtime_acceptance': 'NOT INFERRED',
               'os_delivery_inferred': False, 'subsystem_category_available': False,
               'raw_log_saved': False, 'app_or_vpn_action': False, 'bound_pid': None,
               'actual_device_pid_name_binding_verified': False,
               'limits': ['Root must independently map PID to the exact installed signed candidate.',
                          'Prebinding records remain bounded typed RAM only; only verified exact-PID rows are persisted.',
                          'Relay drops subsystem/category/event framing; text alone never proves native callback or OS delivery.',
                          'Missing/zero rows remain NOT OBSERVED; no automatic follow to a replacement PID.']}
    child = None
    selector = selectors.DefaultSelector()
    pending = []
    counts = collections.Counter()
    buffer = b''
    dropping_oversized = False
    started = time.monotonic()
    next_check = started
    bound_pid = None
    connected_reported = False
    written = discarded = 0
    def stop_capture(_signum, _frame):
        raise KeyboardInterrupt
    previous_terminate = signal.signal(signal.SIGTERM, stop_capture)
    try:
        with os.fdopen(fd, 'w') as output:
            def persist(observation):
                nonlocal written
                output.write(json.dumps(observation, separators=(',', ':')) + '\n')
                output.flush()
                written += 1
                counts[observation['receipt']['stage']] += 1
            child = subprocess.Popen([tool, '-u', args.device_udid, '--no-colors', '--exit',
                                      '-p', 'Runner', '-m', 'apns stage='], stdout=subprocess.PIPE,
                                     stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL)
            selector.register(child.stdout, selectors.EVENT_READ)
            os.set_blocking(child.stdout.fileno(), False)
            print(json.dumps({'capture_state': 'PASSIVE_RELAY_CHILD_STARTED_WAITING_FOR_PID',
                              'runtime_acceptance': 'NOT INFERRED'}), flush=True)
            summary['stop_reason'] = 'DURATION_REACHED'
            while time.monotonic() - started < args.duration:
                if child.poll() is not None:
                    summary['stop_reason'] = 'RELAY_EXITED_OR_DEVICE_DISCONNECTED'
                    summary['relay_exit_code'] = child.returncode
                    break
                if bound_pid is None:
                    if time.monotonic() - started >= args.bind_timeout:
                        summary['stop_reason'] = 'EXACT_PID_NOT_BOUND_BEFORE_DEADLINE'
                        break
                    candidate = read_binding(args.pid_file)
                    if candidate is not None:
                        if not check_pid(tool, args.device_udid, candidate):
                            summary['stop_reason'] = 'SUPPLIED_PID_NOT_CURRENT_RUNNER'
                            break
                        bound_pid = candidate
                        summary['bound_pid'] = candidate
                        summary['actual_device_pid_name_binding_verified'] = True
                        for observation in select_bound(pending, bound_pid):
                            persist(observation)
                        pending.clear()
                        next_check = time.monotonic() + 3
                        print(json.dumps({'capture_state': 'EXACT_RUNNER_PID_BOUND',
                                          'runtime_acceptance': 'NOT INFERRED'}), flush=True)
                elif time.monotonic() >= next_check:
                    if not check_pid(tool, args.device_udid, bound_pid):
                        summary['stop_reason'] = 'BOUND_PID_LOST_OR_UNAVAILABLE'
                        break
                    next_check = time.monotonic() + 3
                for key, _ in selector.select(timeout=0.25):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        continue
                    buffer += chunk
                    while b'\n' in buffer:
                        raw, buffer = buffer.split(b'\n', 1)
                        if dropping_oversized or len(raw) > 65536:
                            dropping_oversized = False; discarded += 1; continue
                        # This is merely a real tool status text observation;
                        # no device id is printed and no callback is inferred.
                        if raw.startswith(b'[connected:') and not connected_reported:
                            connected_reported = True
                            print(json.dumps({'capture_state': 'DEVICE_RELAY_CONNECTION_TEXT_OBSERVED',
                                              'runtime_acceptance': 'NOT INFERRED'}), flush=True)
                        try:
                            parsed = pending_observation(raw.decode('utf-8'))
                        except UnicodeDecodeError:
                            parsed = None
                        if parsed is None:
                            discarded += 1; continue
                        pid, observation = parsed
                        if bound_pid is None:
                            if len(pending) >= 128:
                                raise ValueError('PREBIND_TYPED_BUFFER_LIMIT_REACHED')
                            pending.append((pid, observation))
                        elif pid == bound_pid:
                            persist(observation)
                        else:
                            discarded += 1
                    if len(buffer) > 65536:
                        buffer = b''; dropping_oversized = True
    except KeyboardInterrupt:
        summary['stop_reason'] = 'OPERATOR_INTERRUPTED_PASSIVE_CAPTURE'
    except ValueError:
        summary['stop_reason'] = 'INVALID_BINDING_OR_TYPED_BUFFER_LIMIT'
    except (OSError, subprocess.SubprocessError):
        summary['stop_reason'] = 'PASSIVE_CAPTURE_UNAVAILABLE_NO_RAW_DETAILS_EXPORTED'
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
                       text_receipt_count=written, discarded_lines=discarded,
                       unbound_typed_records_discarded=len(pending), stage_counts=dict(counts))
        with os.fdopen(summary_fd, 'w') as output:
            json.dump(summary, output, indent=2); output.write('\n')
    print(json.dumps({'text_receipt_count': written, 'stop_reason': summary['stop_reason'],
                      'runtime_acceptance': 'NOT INFERRED', 'os_delivery_inferred': False}), flush=True)


if __name__ == '__main__':
    main()
