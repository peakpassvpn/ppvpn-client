#!/usr/bin/env python3
"""Fail closed when a bridge upgrade needs new iOS dynamic lookup symbols."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import unquote, urljoin, urlparse

app = Path(__file__).resolve().parents[1]
root = app.parents[1]
runner_list = app / 'runner/ios/Runner/Bridge.exports'
tunnel_list = app / 'runner/ios/PacketTunnel/Tunnel.exports'

def names(path):
    return set(path.read_text().splitlines())

INTERNAL_GETTER = '_ppvpn_tunnel_internal_packet_diagnostics'


def tunnel_contract(header_path, public_list, internal):
    expected = names(public_list)
    # The normal whitelist must NEVER acquire a diagnostic entry, even when
    # this check is executing as part of an explicitly internal build.
    if INTERNAL_GETTER in expected:
        raise SystemExit('internal getter must not be in public Tunnel.exports')
    if internal:
        expected = expected | {INTERNAL_GETTER}
    # Observe the actual C conditional declaration. Raw regex over both sides
    # of #ifdef incorrectly required the internal getter in ordinary builds.
    define = '-DPPVPN_INTERNAL_DIAGNOSTICS' if internal else '-UPPVPN_INTERNAL_DIAGNOSTICS'
    header = subprocess.check_output(
        ['xcrun', 'clang', '-E', '-P', '-x', 'c', define, str(header_path)], text=True)
    required = {'_' + n for n in re.findall(r'\b(ppvpn_tunnel_[a-z_]+)\s*\(', header)}
    if not required or required != expected:
        raise SystemExit(f'Tunnel export contract changed: missing={required - expected}, stale={expected - required}')
    return expected


def main():
    config = app / '.dart_tool/package_config.json'
    package = next(p for p in json.loads(config.read_text())['packages']
                   if p['name'] == 'flutter_rust_bridge')
    package_root = Path(unquote(urlparse(urljoin(config.as_uri(), package['rootUri'])).path))
    bindings = (package_root / 'lib/src/ffigen_generated/multi_package.dart').read_text()
    bindings += (app / 'lib/src/rust/frb_generated.io.dart').read_text()
    required = {'_' + n for n in re.findall(r"'((?:frb_|store_dart)[A-Za-z0-9_]+)'", bindings)}
    if not required or required != names(runner_list):
        raise SystemExit(f'FRB export contract changed: missing={required - names(runner_list)}, stale={names(runner_list) - required}')
    tunnel_expected = tunnel_contract(root / 'crates/tunnel-host/include/ppvpn_tunnel.h',
                                      tunnel_list, os.environ.get('PPVPN_INTERNAL_DIAGNOSTICS') == '1')
    if len(sys.argv) not in (1, 3):
        raise SystemExit('usage: check-ios-exports.py [Runner PacketTunnel]')
    for binary, expected in zip(sys.argv[1:], (names(runner_list), tunnel_expected)):
        output = subprocess.check_output(['xcrun', 'dyld_info', '-exports', binary], text=True)
        actual = {line.split()[-1] for line in output.splitlines() if re.match(r'\s+0x[0-9A-Fa-f]+\s', line)}
        if actual != expected:
            raise SystemExit(f'{binary}: missing={expected - actual}, unexpected={actual - expected}')
    print('iOS export contracts verified')


if __name__ == '__main__':
    main()
