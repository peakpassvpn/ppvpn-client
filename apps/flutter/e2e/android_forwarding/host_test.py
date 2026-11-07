from email.message import Message
from io import BytesIO
from pathlib import Path
import json
import socket
import threading
import tempfile
import unittest
from unittest.mock import patch

from host import after_control_receipt, ControlRequestError, read_control_body, Evidence, PACKAGE, device_vm_identity, owned_internet_sockets, owned_listeners, profile


class ControlFramingTests(unittest.TestCase):
    def headers(self, length=None):
        headers = Message()
        if length is not None:
            headers['Content-Length'] = str(length)
        return headers

    def test_empty_object_and_fragmented_body_are_valid(self):
        class Fragmented(BytesIO):
            def read(self, length):
                return super().read(min(length, 1))
        self.assertEqual({}, read_control_body(Fragmented(b'{}'), self.headers(2)))

    def test_aborted_release_body_is_reported_as_truncated_not_empty_json(self):
        with self.assertRaisesRegex(ControlRequestError, 'expected=2 received=0'):
            read_control_body(BytesIO(b''), self.headers(2))
        with self.assertRaisesRegex(ControlRequestError, 'expected=2 received=1'):
            read_control_body(BytesIO(b'{'), self.headers(2))

    def test_invalid_or_ambiguous_framing_and_nonobjects_are_rejected(self):
        for length in [None, 0, -1, 1048577, 'bad']:
            with self.subTest(length=length), self.assertRaises(ControlRequestError):
                read_control_body(BytesIO(b'{}'), self.headers(length))
        duplicate = self.headers(2)
        duplicate['Content-Length'] = '2'
        chunked = self.headers(2)
        chunked['Transfer-Encoding'] = 'chunked'
        for headers in [duplicate, chunked]:
            with self.assertRaises(ControlRequestError):
                read_control_body(BytesIO(b'{}'), headers)
        for payload in [b'[]', b'xx', b'\xff\xff']:
            with self.assertRaises(ControlRequestError):
                read_control_body(BytesIO(payload), self.headers(len(payload)))


class ControlReceiptTests(unittest.TestCase):
    def exchange(self, *, receipt_bytes=None, close_client=True, timeout=1):
        action = threading.Event()
        errors = []
        receipt = 'a' * 32
        data = json.dumps({'takeover_armed': True, 'receipt': receipt}).encode()
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            listener.listen(1)
            with socket.create_connection(listener.getsockname()) as client:
                peer, _ = listener.accept()
                with peer:
                    def serve():
                        try:
                            peer.sendall(data)
                            after_control_receipt(peer, lambda: None, action.set,
                                                  receipt, timeout=timeout)
                        except Exception as error:
                            errors.append(error)
                    worker = threading.Thread(target=serve, daemon=True)
                    worker.start()
                    received = bytearray()
                    while len(received) < len(data):
                        received.extend(client.recv(len(data) - len(received)))
                    self.assertTrue(json.loads(received)['takeover_armed'])
                    self.assertFalse(action.is_set())
                    if receipt_bytes is not None:
                        client.sendall(receipt_bytes)
                    if close_client:
                        client.shutdown(socket.SHUT_WR)
                    worker.join(2)
                    self.assertFalse(worker.is_alive())
        return action.is_set(), errors

    def test_real_tcp_explicit_receipt_and_client_close_precede_action(self):
        performed, errors = self.exchange(receipt_bytes=b'a' * 32 + b'\n')
        self.assertTrue(performed)
        self.assertEqual([], errors)

    def test_unacknowledged_reply_times_out_without_vpn_mutation(self):
        performed, errors = self.exchange(close_client=False, timeout=.05)
        self.assertFalse(performed)
        self.assertIsInstance(errors[0], TimeoutError)

    def test_close_alone_never_authorizes_revoke(self):
        performed, errors = self.exchange()
        self.assertFalse(performed)
        self.assertIsInstance(errors[0], ControlRequestError)

    def test_wrong_or_extra_receipt_never_authorizes_revoke(self):
        for value in [b'b' * 32 + b'\n', b'a' * 32 + b'\nextra']:
            performed, errors = self.exchange(receipt_bytes=value)
            self.assertFalse(performed)
            self.assertIsInstance(errors[0], ControlRequestError)

    def test_receipt_without_client_close_does_not_cut_flush_in_flight(self):
        performed, errors = self.exchange(receipt_bytes=b'a' * 32 + b'\n',
                                          close_client=False, timeout=.05)
        self.assertFalse(performed)
        self.assertIsInstance(errors[0], TimeoutError)

    def test_armed_reply_alone_never_establishes_or_proves_revoke(self):
        with tempfile.TemporaryDirectory() as work:
            evidence = Evidence('emulator-test', Path(work), Path(work) / 'node.jsonl')
            evidence.connected = {0, 1, 2}
            evidence.ended = {0, 1}
            with patch.object(evidence, 'adb') as adb, patch.object(evidence, 'shell', return_value='established'):
                armed = evidence.action('/revoke', {})
                self.assertTrue(armed['takeover_armed'])
                self.assertRegex(armed['receipt'], r'^[0-9a-f]{32}$')
                adb.assert_not_called()
                self.assertFalse(evidence.revoke_established)
                snapshot = {'processes': [{'name': PACKAGE, 'listeners': [], 'tun_interfaces': []}],
                            'links': '', 'routes': ''}
                with patch.object(evidence, 'snapshot', return_value=snapshot), patch('host.baseline', return_value={}):
                    with self.assertRaisesRegex(AssertionError, 'No receipt-ordered rival establish proof'):
                        evidence.checkpoint({'stage': 'revoked', 'cycle': 2})
                self.assertEqual({0, 1}, evidence.ended)
                evidence.establish_rival_after_receipt()
                self.assertTrue(evidence.revoke_established)
                self.assertEqual(2, adb.call_count)
                self.assertTrue(json.loads((Path(work) / 'revoke-establishment.json').read_text())['explicit_reply_receipt_verified'])


class DeviceVmTests(unittest.TestCase):
    def process(self):
        return {'pid': 5951, 'start_ticks': 123400, 'start_epoch': 1000.0,
                'listeners': [{'address': '0100007F', 'port': 46405, 'inode': '63297'}]}

    def test_device_publication_matches_same_pid_lifetime_and_fd(self):
        log = '1001.545 5951 6043 I flutter : The Dart VM service is listening on http://127.0.0.1:46405/test-auth=/\n'
        value = device_vm_identity(log, self.process())
        self.assertEqual((5951, 123400, 46405, '63297'),
                         (value['pid'], value['start_ticks'], value['port'], value['inode']))

    def test_stale_pid_other_pid_host_port_and_wildcard_cannot_authorize(self):
        for stamp, pid, host, port in [(999.0, 5951, '127.0.0.1', 46405),
                                      (1001.0, 5950, '127.0.0.1', 46405),
                                      (1001.0, 5951, '127.0.0.1', 55555),
                                      (1001.0, 5951, '0.0.0.0', 46405)]:
            log = f'{stamp:.3f} {pid} 6043 I flutter : The Dart VM service is listening on http://{host}:{port}/auth=/\n'
            with self.subTest(log=log), self.assertRaises(AssertionError):
                device_vm_identity(log, self.process())

    def test_published_device_vm_does_not_authorize_an_extra_listener(self):
        process = self.process()
        process['listeners'].append({'address': '0100007F', 'port': 1080, 'inode': 'proxy'})
        log = '1001.545 5951 6043 I flutter : The Dart VM service is listening on http://127.0.0.1:46405/auth=/\n'
        identity = device_vm_identity(log, process)
        self.assertFalse(all(all(listener[key] == identity[key] for key in ('port', 'inode', 'address'))
                             for listener in process['listeners']))

    def test_reused_pid_or_new_vm_socket_lifetime_is_not_silently_allowed(self):
        with tempfile.TemporaryDirectory() as work:
            evidence = Evidence('emulator-test', Path(work), Path(work) / 'node.jsonl')
            evidence.vm_identity = {'pid': 5951, 'start_ticks': 123400, 'port': 46405,
                                    'address': '0100007F', 'inode': '63297'}
            process = self.process()
            process['start_ticks'] += 100
            with self.assertRaises(AssertionError):
                evidence.vm_for(process)


class HostEvidenceTests(unittest.TestCase):
    def setUp(self):
        probe = patch('host.baseline', return_value={'resolvers': []})
        probe.start()
        self.addCleanup(probe.stop)

    def test_namespace_table_must_intersect_pid_socket_inodes(self):
        table = '''  sl  local_address rem_address st tx rx tr tm retr inode
0: 0100007F:1234 00000000:0000 0A 0:0 00:0 0 10000 0 111
1: 00000000:1080 00000000:0000 0A 0:0 00:0 0 10000 0 222
2: 0100007F:4321 00000000:0000 01 0:0 00:0 0 10000 0 333
'''
        result = owned_listeners([table], 'socket:[111]\nsocket:[333]')
        self.assertEqual([{'address': '0100007F', 'port': 0x1234, 'inode': '111'}], result)
        self.assertEqual(2, len(owned_internet_sockets({'tcp': table}, 'socket:[111]\nsocket:[333]')))

    def test_non_vm_app_listener_and_retained_tun_fail_closed(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            node = root / 'node.jsonl'
            node.write_text('')
            evidence = Evidence('emulator-test', root, node)
            process = {'name': PACKAGE, 'listeners': [], 'tun_interfaces': []}
            evidence.snapshot = lambda _: {'processes': [process], 'links': ''}
            evidence.vm_for = lambda _: {'port': 12345, 'inode': 'vm', 'address': '0100007F'}
            body = {'stage': 'initial-off', 'vm_port': 12345}
            evidence.checkpoint(body)
            process['listeners'] = [{'address': '00000000', 'port': 12345, 'inode': 'vm'}]
            with self.assertRaises(AssertionError):
                evidence.checkpoint(body)
            process['listeners'] = [{'address': '0100007F', 'port': 1080, 'inode': 'proxy'}]
            with self.assertRaises(AssertionError):
                evidence.checkpoint(body)
            process['listeners'] = []
            process['tun_interfaces'] = ['tun0']
            with self.assertRaises(AssertionError):
                evidence.checkpoint(body)

    def test_empty_smoke_cannot_pass_and_profile_has_real_proxy_final(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            evidence = Evidence('emulator-test', root, root / 'node.jsonl')
            with self.assertRaises(AssertionError):
                evidence.action('/complete', {})
            self.assertFalse((root / 'PASS.json').exists())
            value = profile()
            self.assertEqual('proxy', value['routing']['final']['type'])
            self.assertTrue(value['nodes'][0]['capabilities']['udp'])
            self.assertEqual('10.0.2.2', value['nodes'][0]['ingresses'][0]['endpoint']['domain'])

    def test_off_checkpoint_rejects_retained_owner_socket_or_route(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            evidence = Evidence('emulator-test', root, root / 'node.jsonl')
            app = {'name': PACKAGE, 'listeners': [], 'tun_interfaces': []}
            owner = {'name': PACKAGE + ':tunnel', 'listeners': [],
                     'tun_interfaces': [], 'internet_sockets': [{'inode': 'retained'}]}
            state = {'processes': [app, owner], 'links': '', 'routes': ''}
            evidence.snapshot = lambda _: state
            body = {'stage': 'initial-off', 'vm_port': 12345}
            with self.assertRaises(AssertionError):
                evidence.checkpoint(body)
            owner['internet_sockets'] = []
            evidence.owned_interfaces.add('tun0')
            state['routes'] = '192.0.2.0/24 dev tun0 table 100'
            with self.assertRaises(AssertionError):
                evidence.checkpoint(body)


if __name__ == '__main__':
    unittest.main()
