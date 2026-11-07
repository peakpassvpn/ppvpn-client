import copy
import unittest

from sign_ios_archive import prepare_background_modes


class ArchiveBackgroundModes(unittest.TestCase):
    def test_enabled_unsigned_archive_gains_required_mode(self):
        info = {'CFBundleVersion': '344', 'PPVPNApplePushEnabled': False}
        prepare_background_modes(info, True)
        self.assertEqual(info['UIBackgroundModes'], ['remote-notification'])
        self.assertEqual(info['CFBundleVersion'], '344')

    def test_existing_modes_preserved_without_duplicating_remote(self):
        for modes in [['fetch', 'audio'], ['fetch', 'remote-notification']]:
            info = {'UIBackgroundModes': modes.copy()}
            prepare_background_modes(info, True)
            self.assertEqual(info['UIBackgroundModes'].count('remote-notification'), 1)
            self.assertEqual(info['UIBackgroundModes'][:len(modes)], modes)

    def test_disabled_path_does_not_add_or_change_modes(self):
        for info in [{}, {'UIBackgroundModes': ['fetch']}]:
            original = copy.deepcopy(info)
            prepare_background_modes(info, False)
            self.assertEqual(info, original)

    def test_enabled_malformed_modes_fail_before_modification(self):
        for value in ['fetch', None, ['fetch', 1]]:
            info = {'UIBackgroundModes': value}
            original = copy.deepcopy(info)
            with self.assertRaises(ValueError):
                prepare_background_modes(info, True)
            self.assertEqual(info, original)


if __name__ == '__main__':
    unittest.main()
