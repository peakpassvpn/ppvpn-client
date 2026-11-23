#!/usr/bin/env python3
"""Fail closed when a bridge upgrade needs new iOS dynamic lookup symbols."""
import json
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

config = app / '.dart_tool/package_config.json'
package = next(p for p in json.loads(config.read_text())['packages']
               if p['name'] == 'flutter_rust_bridge')
package_root = Path(unquote(urlparse(urljoin(config.as_uri(), package['rootUri'])).path))
bindings = (package_root / 'lib/src/ffigen_generated/multi_package.dart').read_text()
bindings += (app / 'lib/src/rust/frb_generated.io.dart').read_text()
required = {'_' + n for n in re.findall(r"'((?:frb_|store_dart)[A-Za-z0-9_]+)'", bindings)}
if not required or required != names(runner_list):
    raise SystemExit(f'FRB export contract changed: missing={required - names(runner_list)}, stale={names(runner_list) - required}')
header = (root / 'crates/tunnel-host/include/ppvpn_tunnel.h').read_text()
required = {'_' + n for n in re.findall(r'\b(ppvpn_tunnel_[a-z_]+)\s*\(', header)}
if not required or required != names(tunnel_list):
    raise SystemExit('Tunnel export contract changed; update Tunnel.exports')
if len(sys.argv) not in (1, 3):
    raise SystemExit('usage: check-ios-exports.py [Runner PacketTunnel]')
for binary, expected in zip(sys.argv[1:], (names(runner_list), names(tunnel_list))):
    output = subprocess.check_output(['xcrun', 'dyld_info', '-exports', binary], text=True)
    actual = {line.split()[-1] for line in output.splitlines() if re.match(r'\s+0x[0-9A-Fa-f]+\s', line)}
    if actual != expected:
        raise SystemExit(f'{binary}: missing={expected - actual}, unexpected={actual - expected}')
print('iOS export contracts verified')
