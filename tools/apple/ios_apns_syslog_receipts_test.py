import json
import unittest
from ios_apns_syslog_receipts import parse_syslog_line, pidlist_matches


class SyslogBoundaries(unittest.TestCase):
    def line(self, process='Runner', pid=4242, image='', level='Info', stage='tap_received'):
        return (f'Nov  7 22:15:01.123456 {process}{image}[{pid}] <{level}>: '
                f'apns stage={stage} accepted=0 current=1 opened=1 remote=1 domain=none code=0')

    def test_observed_ostrace_header_shape_and_fixed_schema(self):
        result = parse_syslog_line(self.line(), 4242)
        self.assertEqual(result['receipt']['stage'], 'tap_received')
        self.assertEqual(result['observation_kind'], 'PROCESS_BOUND_FIXED_TRACE_TEXT')
        self.assertFalse(result['subsystem_category_verified'])
        self.assertFalse(result['os_delivery_inferred'])
        self.assertNotIn('subsystem', result)
        self.assertNotIn('category', result)
        self.assertNotIn('eventMessage', result)

    def test_exact_pid_and_process_not_name_prefix_or_continuation(self):
        for line in [self.line(pid=4243), self.line(process='RunnerAlt'),
                     self.line(process='PacketTunnel'), '\t' + self.line(),
                     self.line().split(': ', 1)[1]]:
            self.assertIsNone(parse_syslog_line(line, 4242))

    def test_main_image_and_info_only_not_flutter_framework_or_debug(self):
        self.assertIsNotNone(parse_syslog_line(self.line(image='(Runner)'), 4242))
        for image in ['(Flutter)', '(CFNetwork)', '(Runner.debug.dylib)']:
            self.assertIsNone(parse_syslog_line(self.line(image=image), 4242))
        for level in ['Debug', 'Notice', 'Error', 'Fault']:
            self.assertIsNone(parse_syslog_line(self.line(level=level), 4242))

    def test_unknown_stage_domain_extra_body_url_or_newline_rejected(self):
        for line in [self.line(stage='unknown'), self.line() + ' token=SECRET',
                     self.line() + '\nSECRET_BODY', self.line().replace('domain=none', 'domain=https://SECRET'),
                     self.line().replace('domain=none code=0', 'domain=none code=5')]:
            self.assertIsNone(parse_syslog_line(line, 4242))

    def test_invalid_or_colored_or_unobserved_header_rejected(self):
        for line in ['\x1b[31m' + self.line(), self.line().replace('.123456', ''),
                     self.line().replace('22:15', '99:15'), self.line().replace('Nov', 'Fake'),
                     '[connected: SECRET_DEVICE]', 'SECRET_BODY apns stage=tap_received']:
            self.assertIsNone(parse_syslog_line(line, 4242))

    def test_process_table_binding_exact_duplicate_and_wrong_name_rejected(self):
        self.assertTrue(pidlist_matches(b'100 Other\n4242 Runner\n900 Another\n', 4242))
        for raw in [b'4243 Runner\n', b'4242 RunnerAlt\n', b'4242 Runner\n4242 Runner\n', b'4242 /private/Runner\n', b'']:
            self.assertFalse(pidlist_matches(raw, 4242))

    def test_sensitive_header_values_never_saved(self):
        result = parse_syslog_line(self.line(), 4242)
        encoded = json.dumps(result)
        for forbidden in ['4242', 'Runner', '22:15', '123456']:
            self.assertNotIn(forbidden, encoded)


if __name__ == '__main__':
    unittest.main()
