#!/usr/bin/env python3
"""Host-side evidence and assertions for a disposable, rooted emulator.

The controller is loopback-only (ADB reverse, independent of the VPN). Never run against a
physical device. No success is emitted without all three traffic/lifecycle legs.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

from dns_baseline import baseline

PACKAGE = 'com.peakpassvpn.ppvpn'
RIVAL = PACKAGE + '.acceptance.revoke'
TARGET = '192.0.2.10'
LOOPBACK_HEX = ('0100007F', '00000000000000000000000001000000')


class ControlRequestError(ValueError):
    pass


def read_control_body(stream, headers):
    """Reject incomplete framing; never turn an aborted request into {}."""
    lengths = headers.get_all('Content-Length', [])
    if len(lengths) != 1 or headers.get('Transfer-Encoding') is not None:
        raise ControlRequestError('Need one Content-Length and no Transfer-Encoding')
    try:
        length = int(lengths[0])
    except ValueError as error:
        raise ControlRequestError('Invalid Content-Length') from error
    if not 0 < length <= 1024 * 1024:
        raise ControlRequestError(f'Invalid body length: {length}')
    data = bytearray()
    while len(data) < length:
        chunk = stream.read(length - len(data))
        if not chunk:
            raise ControlRequestError(f'Truncated control body: expected={length} received={len(data)}')
        data.extend(chunk)
    try:
        body = json.loads(data)
    except (ValueError, UnicodeError) as error:
        raise ControlRequestError(f'Invalid JSON control body: bytes={len(data)}') from error
    if not isinstance(body, dict):
        raise ControlRequestError('Control body must be an object')
    return body


def after_control_receipt(connection, flush, action, receipt, *, timeout=5):
    """Require the App's explicit receipt and close before cutting its VPN."""
    assert re.fullmatch(r'[0-9a-f]{32}', receipt)
    expected = (receipt + '\n').encode('ascii')
    flush()
    connection.settimeout(timeout)
    received = bytearray()
    try:
        while chunk := connection.recv(len(expected) + 1 - len(received)):
            received.extend(chunk)
            if len(received) > len(expected):
                raise ControlRequestError('Oversized control receipt')
    except ConnectionResetError:
        pass  # A close/reset counts only after the exact explicit receipt.
    if received != expected:
        raise ControlRequestError('Missing or invalid armed-reply receipt')
    action()


def device_vm_identity(log, process):
    """Bind a device VM publication to this process lifetime and socket FD.

    Service.getInfo may advertise the HOST DDS URI. Never authorize that
    port, an adb-forward port, a stale/reused PID or arbitrary loopback socket.
    """
    pattern = re.compile(
        r'^\s*(\d+\.\d+)\s+(\d+)\s+\d+\s+I\s+flutter\s*:\s*'
        r'The Dart VM service is listening on (http://\S+)\s*$')
    candidates = {}
    for line in log.splitlines():
        match = pattern.match(line)
        if not match or int(match[2]) != process['pid']:
            continue
        stamp = float(match[1])
        if stamp < process['start_epoch']:
            continue
        url = urlsplit(match[3])
        if url.hostname not in ('127.0.0.1', '::1') or not url.port:
            continue
        for listener in process['listeners']:
            if listener['port'] == url.port and listener['address'] in LOOPBACK_HEX:
                candidates[(url.port, listener['inode'])] = {
                    'pid': process['pid'], 'start_ticks': process['start_ticks'],
                    'start_epoch': process['start_epoch'], 'device_log_epoch': stamp,
                    'device_url': match[3], 'port': url.port,
                    'inode': listener['inode'], 'address': listener['address'],
                }
    assert len(candidates) == 1, 'Need one live device-published VM listener from this App lifetime'
    return next(iter(candidates.values()))


def profile():
    return {
        'schema_version': 1, 'revision': '7' * 64,
        'generated_at': '2026-01-01T00:00:00Z',
        'expires_at': '2099-01-01T00:00:00Z',
        'nodes': [{
            'id': 'android-node', 'name': 'Android acceptance',
            'entry_key': 'standard', 'exit': {'ip': '203.0.113.10', 'region': 'test'},
            'capabilities': {'tcp': True, 'udp': True},
            'ingresses': [{
                'role': 'primary', 'endpoint_key': 'android-ss', 'replica_ordinal': 0,
                'protocol': 'shadowsocks',
                # Same domain-shaped numeric endpoint as existing Linux e2e.
                'endpoint': {'domain': '10.0.2.2', 'port': 18388},
                'credentials': {'shadowsocks': {
                    'method': '2022-blake3-aes-128-gcm',
                    'user_key': 'AAAAAAAAAAAAAAAAAAAAAA=='}},
                'capabilities': {'tcp': True, 'udp': True},
            }],
        }],
        'selection': {'mode': 'manual', 'default_node_id': 'android-node'},
        'routing': {'rules': [], 'final': {'type': 'proxy', 'target': 'selected'}},
    }


def link_indexes(text):
    return {match[2]: int(match[1]) for match in re.finditer(
        r'^(\d+): ([^:@\s]+)(?:[:@])', text, re.M)}


def owned_listeners(tables, links):
    """A /proc/net table is namespace-wide: intersect with this PID's FDs."""
    inodes = set(re.findall(r'socket:\[(\d+)\]', links))
    result = []
    for table in tables:
        for row in table.splitlines()[1:]:
            fields = row.split()
            if len(fields) < 10 or fields[3] != '0A' or fields[9] not in inodes:
                continue
            address, port = fields[1].split(':')
            result.append({'address': address, 'port': int(port, 16), 'inode': fields[9]})
    return result


def owned_internet_sockets(tables, links):
    inodes = set(re.findall(r'socket:\[(\d+)\]', links))
    return [{'protocol': protocol, 'local': fields[1], 'remote': fields[2],
             'state': fields[3], 'inode': fields[9]}
            for protocol, table in tables.items()
            for row in table.splitlines()[1:]
            if len(fields := row.split()) >= 10 and fields[9] in inodes]


class Evidence:
    def __init__(self, serial, directory, node_log):
        self.serial, self.directory, self.node_log = serial, directory, node_log
        self.run = uuid.uuid4().hex
        self.connected = set()
        self.ended = set()
        self.owned_interfaces = set()
        self.owned_links = set()
        self.route_offset = 0
        self.complete = False
        self.revoke_armed = False
        self.revoke_receipt = None
        self.revoke_established = False
        self.revoke_observed = False
        self.vm_identity = None
        self.lock = threading.Lock()

    def adb(self, *arguments):
        return subprocess.check_output(['adb', '-s', self.serial, *arguments],
                                       timeout=20, text=True, stderr=subprocess.STDOUT)

    def shell(self, script):
        return self.adb('shell', script).replace('\r', '')

    def snapshot(self, stage):
        ps = self.shell('ps -A -o PID,NAME')
        clock_ticks = int(self.shell('getconf CLK_TCK').strip())
        assert clock_ticks > 0
        boot = int(re.search(r'^btime (\d+)$', self.shell('cat /proc/stat'), re.M)[1])
        processes = []
        for row in ps.splitlines()[1:]:
            parts = row.split()
            if len(parts) == 2 and parts[1] in (PACKAGE, PACKAGE + ':tunnel'):
                pid, name = parts
                if not pid.isdigit():
                    raise AssertionError('Invalid PID')
                links = self.shell(f'ls -l /proc/{pid}/fd')
                tables = {kind: self.shell(f'cat /proc/{pid}/net/{kind}')
                          for kind in ('tcp', 'tcp6', 'udp', 'udp6')}
                info = self.shell(f'cat /proc/{pid}/fdinfo/* 2>/dev/null || true')
                interfaces = re.findall(r'^iff:\s*(\S+)', info, re.M)
                stat = self.shell(f'cat /proc/{pid}/stat').strip()
                start_ticks = int(stat.rsplit(') ', 1)[1].split()[19])
                processes.append({'pid': int(pid), 'name': name, 'fd_links': links,
                                  'proc_stat': stat, 'start_ticks': start_ticks,
                                  'clock_ticks': clock_ticks, 'boot_epoch': boot,
                                  'start_epoch': boot + start_ticks / clock_ticks,
                                  'fd_info': info,
                                  'tun_interfaces': sorted(set(interfaces)),
                                  'listeners': owned_listeners([tables['tcp'], tables['tcp6']], links),
                                  'internet_sockets': owned_internet_sockets(tables, links)})
        result = {
            'stage': stage, 'processes': processes,
            'links': self.shell('ip -o link show'),
            'addresses': self.shell('ip addr show'),
            'rules': self.shell('ip rule show'),
            'routes': self.shell('ip route show table all'),
            'connectivity': self.shell('dumpsys connectivity'),
        }
        (self.directory / f'{stage}.json').write_text(json.dumps(result, indent=2) + '\n')
        return result

    def vm_for(self, process):
        if self.vm_identity is None:
            log = self.shell(f"logcat -d -v epoch --pid={process['pid']} -s flutter:I")
            self.vm_identity = device_vm_identity(log, process)
            (self.directory / 'device-vm-publication.txt').write_text(log)
            (self.directory / 'device-vm-identity.json').write_text(
                json.dumps(self.vm_identity, indent=2) + '\n')
        vm = self.vm_identity
        assert (process['pid'], process['start_ticks']) == (vm['pid'], vm['start_ticks']), 'App process lifetime changed'
        return vm

    def events(self):
        return [json.loads(line) for line in self.node_log.read_text().splitlines()
                if line.startswith('{')]

    def checkpoint(self, body):
        stage, cycle = body['stage'], body.get('cycle')
        if stage not in ('initial-off', 'connected', 'stopped', 'revoked'):
            raise AssertionError('Unknown checkpoint')
        if cycle is not None and (type(cycle) is not int or cycle not in range(3)):
            raise AssertionError('Invalid cycle')
        (self.directory / f'dns-baseline-{stage}-{cycle}.json').write_text(
            json.dumps(baseline(), indent=2) + '\n')
        evidence = self.snapshot(f'{stage}-{cycle}')
        app = [p for p in evidence['processes'] if p['name'] == PACKAGE]
        owners = [p for p in evidence['processes'] if p['name'] == PACKAGE + ':tunnel']
        assert len(app) == 1, 'Expected exactly one App process'
        # DDS can replace Service.getInfo's URI with a host endpoint. The
        # allowlist comes exclusively from device logcat + PID lifetime + FD.
        vm = self.vm_for(app[0]) if app[0]['listeners'] or self.vm_identity else None
        for listener in app[0]['listeners']:
            assert vm and all(listener[key] == vm[key] for key in ('port', 'inode', 'address')), (
                    'Unexpected App TCP listener', listener)
        assert not app[0]['tun_interfaces'], 'App owns a TUN FD'
        if stage == 'connected':
            assert cycle not in self.connected, 'Duplicate connected checkpoint'
            assert len(owners) == 1, 'Expected one VPN owner process'
            interfaces = owners[0]['tun_interfaces']
            assert len(interfaces) == 1, ('Expected one owned TUN', interfaces)
            self.owned_interfaces.update(interfaces)
            indexes = link_indexes(evidence['links'])
            for name in interfaces:
                assert name in indexes, 'Owned TUN missing kernel link identity'
                self.owned_links.add((name, indexes[name]))
            assert not owners[0]['listeners'], 'VPN owner opened a local TCP proxy'
            observation = body['observation']
            assert observation['backend'] == 'sail' and observation['running']
            assert observation['active_runtime_ids'] == [observation['native_runtime_id']]
            assert observation['native_runtime_id'] is not None
            token = body['token']
            assert token == f'{self.run}-cycle-{cycle}'
            events = self.events()
            # Stream chunks may split a token; join independently recorded bytes.
            tcp = ''.join(e.get('payload', '') for e in events
                          if e.get('event') == 'echo' and e.get('network') == 'tcp')
            udp = [e.get('payload') for e in events
                   if e.get('event') == 'echo' and e.get('network') == 'udp']
            assert token + '-tcp\n' in tcp, 'No controlled upstream TCP bytes'
            assert token + '-udp' in udp, 'No controlled upstream UDP datagram'
            new = events[self.route_offset:]
            assert any(e.get('event') == 'route' and e.get('network') == 'tcp'
                       and e.get('destination', '').endswith(':853') for e in new), (
                           'No real production DoT dial through this SS node in this cycle')
            self.route_offset = len(events)
            self.connected.add(cycle)
        else:
            assert all(not p['tun_interfaces'] for p in owners), 'Stopped host retained TUN FDs'
            for interface in self.owned_interfaces:
                assert not re.search(r'^\d+: ' + re.escape(interface) + r'[:@]',
                                     evidence['links'], re.M), 'Old VPN interface remains'
            assert all(not p['listeners'] for p in owners), 'Stopped host retained listener'
            assert all(not p['internet_sockets'] for p in owners), 'Stopped host retained Internet socket FDs'
            for interface in self.owned_interfaces:
                assert not re.search(r'\bdev ' + re.escape(interface) + r'\b',
                                     evidence['routes']), 'Old VPN routes remain'
            if stage != 'initial-off':
                assert cycle in self.connected and cycle not in self.ended
                assert (stage == 'revoked') == (cycle == 2)
                if stage == 'revoked':
                    assert self.revoke_established, 'No receipt-ordered rival establish proof'
                    assert self.revoke_observed, 'No primary exit witness before rival cleanup'
                self.ended.add(cycle)
        (self.directory / f'{stage}-{cycle}-assertions.json').write_text(
            json.dumps(body, indent=2) + '\n')
        return {'ok': True, 'device_vm': vm}

    def action(self, path, body):
        with self.lock:
            if path == '/profile':
                return {'run': self.run, 'profile': profile()}
            if path == '/authorize-primary':
                assert not self.connected and not self.ended
                self.adb('shell', 'appops', 'set', PACKAGE, 'ACTIVATE_VPN', 'allow')
                return {'ok': True}
            if path == '/checkpoint':
                return self.checkpoint(body)
            if path in ('/dns-baseline', '/diagnostics'):
                cycle = body['cycle']
                assert type(cycle) is int and cycle in range(3)
                if path == '/dns-baseline':
                    value = {'baseline': baseline(), 'owner': body,
                             'os': self.snapshot(f'before-traffic-{cycle}')}
                    name = f'dns-baseline-during-{cycle}.json'
                else:
                    value, name = body, f'dns-failure-{cycle}.json'
                (self.directory / name).write_text(json.dumps(value, indent=2) + '\n')
                return {'ok': True}
            if path == '/revoke':
                assert self.connected == {0, 1, 2} and self.ended == {0, 1}
                assert not self.revoke_armed, 'Duplicate revoke arm'
                self.revoke_armed = True
                self.revoke_receipt = uuid.uuid4().hex
                return {'ok': True, 'takeover_armed': True, 'receipt': self.revoke_receipt}
            if path == '/revoke-observed':
                return self.observe_primary_revoke(body)
            if path == '/release-rival':
                self.adb('shell', 'am', 'force-stop', RIVAL)
                return {'ok': True}
            if path == '/complete':
                assert self.connected == self.ended == {0, 1, 2}
                assert self.revoke_established and self.revoke_observed
                self.complete = True
                (self.directory / 'PASS.json').write_text(json.dumps({
                    'run': self.run, 'cycles': 3, 'tcp_udp_dns': True,
                    'explicit_stop': 2, 'system_revoke': 1,
                    'boundary': 'owner IPC profile; not authenticated App account/UI flow',
                }, indent=2) + '\n')
                return {'ok': True}
            raise AssertionError('Unknown control operation')


    def observe_primary_revoke(self, body):
        assert self.revoke_established and not self.revoke_observed
        assert self.connected == {0, 1, 2} and self.ended == {0, 1}
        assert body['cycle'] == 2 and body['primary_state'] == 'off'
        assert body['owner_socket_exists'] is False and body['tcp_witness_ended'] is True
        evidence = self.snapshot('revoke-observed-2')
        owners = [p for p in evidence['processes'] if p['name'] == PACKAGE + ':tunnel']
        assert all(not p['tun_interfaces'] for p in owners), 'Revoked primary retained TUN FDs'
        assert all(not p['listeners'] for p in owners), 'Revoked primary retained listener'
        assert all(not p['internet_sockets'] for p in owners), 'Revoked primary retained Internet FDs'
        current = set(link_indexes(evidence['links']).values())
        assert self.owned_links, 'Missing prior primary link identities'
        assert all(index not in current for _, index in self.owned_links), 'Old primary kernel link still exists'
        native_log = self.shell(f'run-as {PACKAGE} cat files/tunnel-state/logs/tunnel-host.log')
        (self.directory / 'revoke-owner-native.log').write_text(native_log)
        starts = len(re.findall(r'^\d+ started:', native_log, re.M))
        stops = len(re.findall(r'^\d+ stopped$', native_log, re.M))
        assert starts == stops == 3, 'Native host has not completed all three exits'
        assert 'engine shutdown:' not in native_log, 'Native shutdown reported failure'
        self.revoke_observed = True
        (self.directory / 'revoke-observed-assertions.json').write_text(json.dumps({
            'run': self.run, 'cycle': 2, 'before_rival_cleanup': True,
            'primary_owner_fd_clear': True, 'old_link_ids_absent': sorted(self.owned_links),
            'owner_socket_exists': False, 'tcp_witness_ended': True,
            'native_started': starts, 'native_stopped': stops,
            'registry_direct_after_revoke': None,
            'registry_evidence': 'Owner RPC destroyed; native stop completed without shutdown error',
        }, indent=2) + '\n')
        return {'ok': True, 'primary_exit_verified': True}

    def establish_rival_after_receipt(self):
        with self.lock:
            assert self.revoke_armed and not self.revoke_established
            assert self.connected == {0, 1, 2} and self.ended == {0, 1}
            self.adb('shell', 'appops', 'set', RIVAL, 'ACTIVATE_VPN', 'allow')
            self.adb('shell', 'am', 'start', '-W', '-n', RIVAL + '/.RevokeActivity')
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if self.shell(f'run-as {RIVAL} cat files/established 2>/dev/null || true').strip() == 'established':
                    self.revoke_established = True
                    (self.directory / 'revoke-establishment.json').write_text(json.dumps({
                        'run': self.run, 'control_reply_closed_before_establish': True,
                        'explicit_reply_receipt_verified': True,
                        'system_takeover': True, 'rival_established_marker': True,
                    }, indent=2) + '\n')
                    return
                time.sleep(.1)
            raise AssertionError('Rival did not establish; onRevoke cannot be claimed')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--serial', required=True)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--node-log', type=Path, required=True)
    args = parser.parse_args()
    args.directory.mkdir(parents=True, exist_ok=True)
    state = Evidence(args.serial, args.directory, args.node_log)
    assert state.shell('getprop ro.kernel.qemu').strip() == '1', 'Emulators only'
    assert state.shell('id -u').strip() == '0', 'A disposable rooted emulator is required'

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            body = None
            try:
                body = read_control_body(self.rfile, self.headers)
                result = state.action(self.path, body)
                status = 200
            except Exception as error:
                result = {'error': str(error), 'type': type(error).__name__}
                status = 400 if isinstance(error, ControlRequestError) else 500
                with (args.directory / 'failures.jsonl').open('a') as file:
                    file.write(json.dumps({'path': self.path, 'request': body, **result}) + '\n')
            data = json.dumps(result).encode()
            self.send_response(status)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            if self.path == '/revoke' and status == 200:
                try:
                    after_control_receipt(self.connection, self.wfile.flush,
                                          state.establish_rival_after_receipt, result['receipt'])
                except Exception as error:
                    with (args.directory / 'failures.jsonl').open('a') as file:
                        file.write(json.dumps({'path': self.path,
                                              'stage': 'after-control-receipt',
                                              'error': str(error),
                                              'type': type(error).__name__}) + '\n')

    server = ThreadingHTTPServer(('127.0.0.1', 18090), Handler)
    print('ready', flush=True)
    server.serve_forever()


if __name__ == '__main__':
    main()
