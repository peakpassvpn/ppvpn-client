import json
import os
from pathlib import Path
import tempfile
import unittest
from ios_apns_syslog_prearm import pending_observation, read_binding, select_bound


class PrearmBoundaries(unittest.TestCase):
    def row(self, pid):
        return f'Nov  7 22:15:01.123456 Runner[{pid}] <Info>: apns stage=token_received accepted=1 current=1 opened=0 remote=0 domain=none code=0'

    def test_only_fixed_typed_observations_enter_ram_not_raw_strings(self):
        parsed = pending_observation(self.row(4242))
        self.assertEqual(parsed[0], 4242)
        self.assertNotIn('eventMessage', parsed[1])
        self.assertNotIn('22:15', json.dumps(parsed))
        self.assertIsNone(pending_observation(self.row(4242) + ' token=SECRET'))

    def test_flush_only_exact_bound_pid(self):
        first, second = pending_observation(self.row(4242)), pending_observation(self.row(4243))
        self.assertEqual(select_bound([first, second], 4242), [first[1]])
        self.assertEqual(select_bound([first, second], 9999), [])

    def test_binding_absent_then_exact_private_numeric_file(self):
        with tempfile.TemporaryDirectory() as temp:
            p = Path(temp) / 'runner.pid'
            self.assertIsNone(read_binding(p))
            fd = os.open(p, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            os.write(fd, b'4242\n'); os.close(fd)
            self.assertEqual(read_binding(p), 4242)

    def test_unsafe_or_malformed_binding_never_selects_pid(self):
        with tempfile.TemporaryDirectory() as temp:
            p = Path(temp) / 'runner.pid'
            for value in ['0', '4242 Runner', '4242\nSECRET', 'a' * 100]:
                p.write_text(value); p.chmod(0o600)
                with self.assertRaises(ValueError): read_binding(p)
            p.write_text('4242'); p.chmod(0o644)
            with self.assertRaises(ValueError): read_binding(p)
            link = Path(temp) / 'linked.pid'; link.symlink_to(p)
            with self.assertRaises(ValueError): read_binding(link)


if __name__ == '__main__':
    unittest.main()
