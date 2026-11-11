"""Validate an optional same-source iOS origin pair, without changing runtime inputs."""
import argparse
import json
import math
from pathlib import Path
import re
import time
from urllib.parse import urlsplit


def origin(value):
    if not isinstance(value, str):
        raise ValueError('candidate origin must be a string')
    parsed = urlsplit(value)
    if (parsed.scheme != 'https' or not parsed.hostname or parsed.username is not None
            or parsed.password is not None or parsed.path or parsed.query or parsed.fragment
            or not re.fullmatch(r'[A-Za-z0-9.-]+', parsed.hostname)):
        raise ValueError('candidate origin must be an HTTPS origin without credentials or path')
    _ = parsed.port
    return value


def plan(text, primary, candidate_only, platform, reuse_baseline):
    if not candidate_only or platform != 'ios' or reuse_baseline:
        raise ValueError('paired origins require candidate-only iOS without a baseline')
    value = json.loads(text)
    allowed = {'origin', 'primary_expires_unix', 'secondary_expires_unix'}
    if not isinstance(value, dict) or set(value) - allowed or 'origin' not in value:
        raise ValueError('invalid paired candidate fields')
    primary, secondary = origin(primary), origin(value['origin'])
    if primary == secondary:
        raise ValueError('paired candidate origins must differ')
    result = {'candidate': {'origin': primary}, 'secondary': {'origin': secondary}}
    for side, key in [('candidate', 'primary_expires_unix'), ('secondary', 'secondary_expires_unix')]:
        if key in value:
            expiry = value[key]
            if isinstance(expiry, bool) or not isinstance(expiry, (int, float)) or not math.isfinite(expiry):
                raise ValueError('candidate expiry must be a finite Unix timestamp')
            result[side]['expires_unix'] = expiry
    return result


def selected(value, side, now, remaining):
    item = value[side]
    origin(item['origin'])
    if item.get('expires_unix', math.inf) - now < remaining:
        raise ValueError('candidate origin has insufficient remaining lifetime')
    return item['origin']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    prepare = sub.add_parser('prepare')
    prepare.add_argument('--json', required=True)
    prepare.add_argument('--primary', required=True)
    prepare.add_argument('--candidate-only', required=True)
    prepare.add_argument('--platform', required=True)
    prepare.add_argument('--reuse-baseline', default='')
    prepare.add_argument('--output', type=Path, required=True)
    choose = sub.add_parser('select')
    choose.add_argument('--manifest', type=Path, required=True)
    choose.add_argument('--side', choices=['candidate', 'secondary'], required=True)
    choose.add_argument('--minimum-remaining', type=int, default=1800)
    args = parser.parse_args()
    if args.command == 'prepare':
        value = plan(args.json, args.primary, args.candidate_only == '1', args.platform, args.reuse_baseline)
        for side in value:
            selected(value, side, time.time(), 1800)
        args.output.write_text(json.dumps(value, indent=2) + '\n')
    else:
        print(selected(json.loads(args.manifest.read_text()), args.side, time.time(), args.minimum_remaining))


if __name__ == '__main__':
    main()
