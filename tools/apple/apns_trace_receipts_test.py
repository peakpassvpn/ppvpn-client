import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from apns_trace_receipts import CATEGORY, SUBSYSTEM, STAGES, parse, parse_settings_message


class PrivacyTests(unittest.TestCase):
    def test_typed_notification_settings_namespace_and_privacy(self):
        line = 'apns_settings phase=explicit authorization=authorized delegate=receiver alert=enabled style=banner center=enabled lock=disabled sound=enabled badge=disabled'
        event = self.event() | {'category': 'settings', 'eventMessage': line}
        self.assertEqual(parse(event, {42})['authorization'], 'authorized')
        self.assertIsNone(parse(event | {'category': CATEGORY}, {42}))
        for text in [line + ' token=SECRET', line.replace('receiver', 'SECRET_CLASS'), line.replace('authorized', 'SECRET')]:
            self.assertIsNone(parse_settings_message(text))
    def event(self, stage='tap_received'):
        return dict(subsystem=SUBSYSTEM, category=CATEGORY, processID=42,
                    eventMessage=f'apns stage={stage} accepted=0 current=1 opened=1 remote=1 domain=none code=0')

    def test_all_fixed_stages_without_extra_metadata(self):
        for stage in STAGES:
            event = self.event(stage)
            event.update(token='SECRET_TOKEN', accountID='SECRET_ACCOUNT', timestamp='SECRET_URL')
            result = parse(event, {42})
            self.assertEqual(result['stage'], stage)
            self.assertNotIn('SECRET', json.dumps(result))
            self.assertEqual(set(result), {'stage', 'accepted', 'current', 'opened', 'remote', 'domain', 'code'})

    def test_untrusted_domain_and_arbitrary_payload_cannot_enter_output(self):
        for message in [self.event()['eventMessage'] + ' token=SECRET',
                        self.event()['eventMessage'].replace('domain=none', 'domain=https://SECRET'),
                        self.event()['eventMessage'] + '\nSECRET', 'SECRET']:
            event = self.event(); event['eventMessage'] = message
            self.assertIsNone(parse(event, {42}))

    def test_wrong_process_subsystem_category_rejected(self):
        for key in ['subsystem', 'category', 'processID']:
            event = self.event(); event[key] = 'wrong'
            self.assertIsNone(parse(event, {42}))
        self.assertIsNone(parse(self.event(), {43}))

    def test_exact_process_path_tracks_cold_pid_without_exporting_path(self):
        path = '/Applications/Test.app/Contents/MacOS/Test'
        event = self.event() | {'processImagePath': path}
        self.assertIsNotNone(parse(event, set(), path))
        event['processID'] = 99
        self.assertIsNotNone(parse(event, set(), path))
        self.assertIsNone(parse(event | {'processImagePath': '/unreviewed'}, set(), path))
        self.assertNotIn(path, json.dumps(parse(event, set(), path)))

    def test_failure_domain_code_and_integer_bounds(self):
        event = self.event('registration_failed')
        event['eventMessage'] = event['eventMessage'].replace('domain=none code=0', 'domain=other code=-42')
        self.assertEqual(parse(event, {42})['code'], -42)
        event['eventMessage'] = event['eventMessage'].replace('-42', str(2**63))
        self.assertIsNone(parse(event, {42}))
        self.assertIsNone(parse(self.event('bridge_notice_reply') | {'eventMessage': self.event()['eventMessage'].replace('code=0', 'code=5')}, {42}))

    def test_cli_no_false_pass_and_private_exclusive_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / 'receipts.ndjson'
            command = [sys.executable, str(Path(__file__).with_name('apns_trace_receipts.py')),
                       '--pid', '42', '--output', str(output)]
            result = subprocess.run(command, input=json.dumps(self.event()) + '\nSECRET\n', text=True, capture_output=True, check=True)
            self.assertEqual(json.loads(result.stdout)['runtime_acceptance'], 'NOT INFERRED')
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)
            before = output.read_bytes()
            self.assertNotIn(b'SECRET', before)
            again = subprocess.run(command, input='', text=True, capture_output=True)
            self.assertNotEqual(again.returncode, 0)
            self.assertEqual(before, output.read_bytes())


if __name__ == '__main__':
    unittest.main()
