from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from host import Evidence, PACKAGE, owned_internet_sockets, owned_listeners, profile


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
            body = {'stage': 'initial-off', 'vm_port': 12345}
            evidence.checkpoint(body)
            process['listeners'] = [{'address': '00000000', 'port': 12345}]
            with self.assertRaises(AssertionError):
                evidence.checkpoint(body)
            process['listeners'] = [{'address': '0100007F', 'port': 1080}]
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
