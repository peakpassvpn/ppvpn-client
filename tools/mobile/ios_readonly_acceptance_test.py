import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
import tempfile
import contextlib
import io

spec = importlib.util.spec_from_file_location('ios_readonly', Path(__file__).with_name('ios_readonly_acceptance.py'))
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class PreflightTests(unittest.TestCase):
    def test_native_log_directory_matches_provider_and_host_sources(self):
        root = Path(__file__).resolve().parents[2]
        provider = (root / 'apps/flutter/runner/ios/PacketTunnel/PacketTunnelProvider.swift').read_text()
        host = (root / 'crates/tunnel-host/src/host.rs').read_text()
        self.assertIn('stateDir: container.appendingPathComponent("tunnel-state").path', provider)
        self.assertIn('let logs = state_dir.join("logs");', host)
        self.assertEqual(check.HOST_LOG_DIR, 'tunnel-state/logs')

    def test_real_collector_command_construction_uses_exact_readonly_paths(self):
        calls = []
        def capture(out, name, args):
            calls.append(args)
            return {} if args[:3] == ['device', 'info', 'files'] else None
        with tempfile.TemporaryDirectory() as directory:
            output = str(Path(directory) / 'capture')
            args = ['collector', '--device', 'ABCDEF0123456789', '--output', output,
                    '--expected-origin', 'https://www.peakpass.dev', '--copy-logs']
            with patch.object(check, 'capture', capture), patch('sys.argv', args), contextlib.redirect_stdout(io.StringIO()):
                check.main()
        listings = [c[c.index('--subdirectory') + 1] for c in calls if '--subdirectory' in c]
        sources = [c[c.index('--source') + 1] for c in calls if '--source' in c]
        self.assertEqual(listings, ['logs', 'tunnel-state/logs'])
        self.assertEqual(sources[:2], ['tunnel-state/logs/tunnel-host.log', 'tunnel-state/logs/ppvpn-core.log'])
        self.assertTrue(all(not s.startswith('data/') for s in sources))
        self.assertTrue(all(c[:3] == ['device', 'copy', 'from'] for c in calls if '--source' in c))
        self.assertEqual(len(sources), 6)

    def test_running_old_provider_and_build_mismatch_never_pass(self):
        apps = {'apps': [{'bundleIdentifier': check.BUNDLE, 'bundleVersion': '21', 'url': 'file:///own/Runner.app/'}]}
        processes = {'runningProcesses': [
            {'executable': 'file:///own/Runner.app/PlugIns/PacketTunnel.appex/PacketTunnel', 'processIdentifier': 5},
            {'executable': 'file:///other/Other.app/Other', 'processIdentifier': 6}]}
        report = check.summarize(apps, processes, {'build': 'final'})
        self.assertEqual(report['provider_process_pids'], [5])
        self.assertEqual(report['app_process_pids'], [])
        self.assertEqual(len(report['preflight_blockers']), 2)
        self.assertEqual(report['runtime_acceptance'], 'NOT RUN')

    def test_failed_query_cannot_be_interpreted_as_no_runtime(self):
        report = check.summarize(None, None)
        self.assertTrue(any('incomplete' in x for x in report['preflight_blockers']))
        self.assertEqual(report['runtime_acceptance'], 'NOT RUN')

    def test_matching_build_still_does_not_claim_listener_or_forwarding_pass(self):
        apps = {'apps': [{'bundleIdentifier': check.BUNDLE, 'bundleVersion': 'final', 'url': 'file:///own/Runner.app/'}]}
        report = check.summarize(apps, {'runningProcesses': []}, {'build': 'final'})
        self.assertEqual(report['preflight_blockers'], [])
        self.assertEqual(report['runtime_acceptance'], 'NOT RUN')
        self.assertFalse(report['active_test_or_install_performed'])


if __name__ == '__main__':
    unittest.main()
