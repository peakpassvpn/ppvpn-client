"""Independent regression cases for incomplete or misleading OS evidence."""
import unittest
import check_macos_capture_acceptance_test as fixtures

ROUTES = fixtures.ROUTES
check = fixtures.check


class IndependentEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.AcceptanceEvidenceTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.directory = self.fixture.directory

    def reject(self, expected):
        with self.assertRaisesRegex(ValueError, expected):
            check.validate(self.directory)

    def test_empty_interface_snapshot_cannot_hide_leaked_utun(self):
        (self.directory / 'interfaces-after.txt').write_text('')
        self.reject('missing interface snapshot')

    def test_missing_ipv6_table_is_not_restoration(self):
        (self.directory / 'routes-after.txt').write_text(ROUTES.split('Internet6:')[0])
        self.reject('routes not restored after')

    def test_changed_ipv6_default_is_not_restoration(self):
        (self.directory / 'routes-after.txt').write_text(ROUTES.replace('fe80::1%en0', 'fe80::2%en0'))
        self.reject('routes not restored after')

    def test_unparsed_utun_route_cannot_be_silently_discarded(self):
        (self.directory / 'routes-after.txt').write_text(ROUTES + 'unparsed-address link#19 UCS utun7\n')
        self.reject('unparsed utun route')

    def test_truncated_route_cannot_be_silently_discarded(self):
        (self.directory / 'routes-after.txt').write_text(ROUTES + '::/1 link#19 UCS\n')
        self.reject('truncated route row')

    def test_ipv6_split_route_leak_is_rejected(self):
        (self.directory / 'routes-after.txt').write_text(ROUTES + '8000::/1 link#19 UCS utun7\n')
        self.reject('routes not restored after')

    def test_consistent_empty_owner_is_not_a_forwarder_identity(self):
        for row in self.fixture.rows[:-1]:
            row['owner'] = {}
        self.fixture.write_events()
        self.reject('invalid forwarding owner')

    def test_boolean_runtime_id_is_not_a_runtime_identity(self):
        for row in self.fixture.rows[:-1]:
            row['runtime_ids'] = [True]
        self.fixture.write_events()
        self.reject('invalid runtime identity')

    def test_boolean_generation_is_not_equal_to_integer_generation(self):
        self.fixture.rows[12]['owner'] = {'instance': 'one-owner', 'generation': True}
        self.fixture.write_events()
        self.reject('invalid forwarding owner')


if __name__ == '__main__':
    unittest.main()
