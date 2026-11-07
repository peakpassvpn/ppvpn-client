#!/usr/bin/env python3
"""Filter unified-log NDJSON from stdin into content-free native APNs receipts.

Never invokes log, launches an app or sends a notification. Requires explicit
observed candidate PIDs; does not store raw events/metadata or infer delivery.
"""
import argparse
import json
import os
import re
import sys

SUBSYSTEM = 'com.peakpassvpn.ppvpn.apple-push'
CATEGORY = 'lifecycle'
STAGES = ('register_requested', 'token_received', 'registration_failed', 'permission_result',
          'foreground_received', 'background_received', 'tap_received',
          'bridge_register_reply', 'bridge_unregister_reply', 'bridge_notice_reply')
LINE = re.compile(r'apns stage=(' + '|'.join(STAGES) +
    r') accepted=([01]) current=([01]) opened=([01]) remote=([01]) '
    r'domain=(none|cocoa|url|os_status|other) code=(-?[0-9]{1,20})')
SETTINGS = re.compile(
    r'apns_settings phase=(explicit|after_permission) '
    r'authorization=(not_determined|denied|authorized|provisional|ephemeral|unknown) '
    r'delegate=(receiver|app_delegate|other|none) '
    r'alert=(unsupported|disabled|enabled|unknown) style=(none|banner|alert|unknown) '
    r'center=(unsupported|disabled|enabled|unknown) lock=(unsupported|disabled|enabled|unknown) '
    r'sound=(unsupported|disabled|enabled|unknown) badge=(unsupported|disabled|enabled|unknown)')


def parse_settings_message(message):
    match = SETTINGS.fullmatch(message) if isinstance(message, str) else None
    if match is None:
        return None
    return dict(stage='notification_settings', **dict(zip(
        ('phase', 'authorization', 'delegate', 'alert', 'style', 'center', 'lock', 'sound', 'badge'), match.groups())))


def parse_message(message):
    """Fixed message grammar only; conveys no source/OS delivery assurance."""
    match = LINE.fullmatch(message) if isinstance(message, str) else None
    if match is None:
        return None
    stage, accepted, current, opened, remote, domain, code = match.groups()
    number = int(code)
    if not -(2**63) <= number < 2**63:
        return None
    if stage not in ('registration_failed', 'permission_result') and (domain != 'none' or number != 0):
        return None
    return dict(stage=stage, accepted=accepted == '1', current=current == '1',
                opened=opened == '1', remote=remote == '1', domain=domain, code=number)


def parse(event, pids, process_path=None):
    if not isinstance(event, dict):
        return None
    if event.get('subsystem') != SUBSYSTEM or event.get('category') not in (CATEGORY, 'settings'):
        return None
    pid = event.get('processID', event.get('processIdentifier'))
    if type(pid) is not int or pid <= 0:
        return None
    if process_path is not None:
        if event.get('processImagePath') != process_path:
            return None
    elif pid not in pids:
        return None
    if event.get('category') == 'settings':
        return parse_settings_message(event.get('eventMessage'))
    return parse_message(event.get('eventMessage'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--pid', type=int, action='append', help='observed candidate PID; repeat after cold launch')
    source.add_argument('--process-path', help='exact reviewed installed executable image path; supports cold PID changes')
    parser.add_argument('--output', required=True, help='new private NDJSON file')
    args = parser.parse_args()
    if any(pid <= 0 for pid in args.pid or []):
        parser.error('candidate PIDs must be positive')
    if args.process_path is not None and not os.path.isabs(args.process_path):
        parser.error('candidate process path must be absolute')
    count = rejected = 0
    counts = {stage: 0 for stage in STAGES}
    fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as output:
        # Bounded reads: malformed/oversized lines are discarded without echo.
        while line := sys.stdin.buffer.readline(65537):
            if len(line) > 65536:
                while line and not line.endswith(b'\n'):
                    line = sys.stdin.buffer.readline(65537)
                rejected += 1
                continue
            try:
                receipt = parse(json.loads(line), set(args.pid or []), args.process_path)
            except (ValueError, UnicodeDecodeError):
                receipt = None
            if receipt is None:
                rejected += 1
                continue
            output.write(json.dumps(receipt, separators=(',', ':')) + '\n')
            output.flush()
            count += 1
            counts[receipt['stage']] = counts.get(receipt['stage'], 0) + 1
    print(json.dumps({'receipt_count': count, 'discarded_lines': rejected, 'stages': counts,
                      'runtime_acceptance': 'NOT INFERRED', 'raw_log_saved': False}))


if __name__ == '__main__':
    main()
