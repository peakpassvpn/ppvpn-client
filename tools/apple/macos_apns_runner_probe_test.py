import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from macos_apns_runner_probe import Refused, outcome, require_hosted_mac, source_hashes


class ProbeBoundaries(unittest.TestCase):
    def event(self, stage, accepted=False):
        return dict(stage=stage, accepted=accepted, current=True, opened=False,
                    remote=False, domain='none', code=0)

    def test_hosted_context_guard_refuses_local_and_self_hosted(self):
        valid = dict(GITHUB_ACTIONS='true', RUNNER_ENVIRONMENT='github-hosted',
                     RUNNER_TEMP='/owned', GITHUB_RUN_ID='1')
        require_hosted_mac(valid, 'darwin')
        for env, platform in [(valid, 'linux'), ({}, 'darwin'),
                               (valid | {'RUNNER_ENVIRONMENT': 'self-hosted'}, 'darwin'),
                               (valid | {'RUNNER_TEMP': ''}, 'darwin')]:
            with self.assertRaises(Refused):
                require_hosted_mac(env, platform)

    def test_token_requires_prior_request_and_same_candidate_process(self):
        request = (42, self.event('register_requested'))
        token = (42, self.event('token_received', True))
        self.assertEqual(outcome([request, token]), 'OS_TOKEN_CALLBACK_OBSERVED')
        self.assertIsNone(outcome([token]))
        self.assertIsNone(outcome([token, request]))
        self.assertIsNone(outcome([request, (43, token[1])]))
        self.assertIsNone(outcome([request, (42, self.event('token_received'))]))

    def test_actual_registration_error_is_distinct_from_no_callback(self):
        error = self.event('registration_failed') | {'domain': 'other', 'code': 42}
        self.assertEqual(outcome([(42, self.event('register_requested')), (42, error)]),
                         'OS_REGISTRATION_ERROR_OBSERVED')
        self.assertIsNone(outcome([(42, error)]))
        self.assertIsNone(outcome([]))

    def test_ui_tap_backend_ack_or_mock_notice_cannot_prove_os_token(self):
        rows = [(42, self.event('register_requested'))]
        for stage in ['foreground_received', 'background_received', 'tap_received', 'bridge_register_reply', 'bridge_notice_reply']:
            self.assertIsNone(outcome(rows + [(42, self.event(stage, True))]))

    def test_only_fixed_source_names_and_hashes_exported(self):
        values = {'apps/flutter/runner/apple/ApplePushState.swift': 'a' * 64,
                  'apps/flutter/runner/macos/Runner/ApplePush.swift': 'b' * 64}
        self.assertEqual(source_hashes({'source_sha256': values, 'token': 'SECRET'}), values)
        for mutated in [values | {'SECRET': 'c' * 64}, values | {next(iter(values)): 'SECRET'}, {}]:
            with self.assertRaises(Refused):
                source_hashes({'source_sha256': mutated})

    def test_local_cli_refuses_before_any_file_or_app_action(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / 'must-not-exist.json'
            result = subprocess.run([sys.executable, str(Path(__file__).with_name('macos_apns_runner_probe.py')),
                '--app', '/not-a-candidate.app', '--verifier', '/not-a-verifier.py',
                '--executable-sha256', 'a' * 64, '--profile-sha256', 'b' * 64,
                '--verifier-sha256', 'c' * 64, '--output', str(output)],
                env=os.environ | {'GITHUB_ACTIONS': 'false'}, capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn('NOT_A_GITHUB_HOSTED_MAC_JOB', result.stderr)
            self.assertFalse(output.exists())

    def test_unsigned_builder_refuses_changed_production_source(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for relative in ['apps/flutter/runner/apple/ApplePushState.swift', 'apps/flutter/runner/macos/Runner/ApplePush.swift']:
                path = root / relative; path.parent.mkdir(parents=True, exist_ok=True); path.write_text('changed source')
            output = root / 'output'
            result = subprocess.run([sys.executable, str(Path(__file__).with_name('build_macos_apns_capability_probe.py')),
                '--source-root', str(root), '--output', str(output), '--state-sha256', 'a' * 64,
                '--receiver-sha256', 'b' * 64, '--build-number', '344', '--arch', 'arm64'],
                capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
