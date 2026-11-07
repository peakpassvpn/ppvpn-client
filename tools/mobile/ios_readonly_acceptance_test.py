import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('ios_readonly', Path(__file__).with_name('ios_readonly_acceptance.py'))
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class PreflightTests(unittest.TestCase):
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
