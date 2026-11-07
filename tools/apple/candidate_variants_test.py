import json
import unittest

from candidate_variants import plan, selected


class CandidatePair(unittest.TestCase):
    def prepare(self, value, **overrides):
        args = dict(primary='https://controlled.example', candidate_only=True, platform='ios', reuse_baseline='')
        args.update(overrides)
        return plan(json.dumps(value), **args)

    def test_same_source_pair_keeps_exact_origins(self):
        value = self.prepare({'origin': 'https://www.peakpass.dev', 'primary_expires_unix': 5000})
        self.assertEqual(selected(value, 'candidate', 1000, 1800), 'https://controlled.example')
        self.assertEqual(selected(value, 'secondary', 10000, 1800), 'https://www.peakpass.dev')

    def test_pair_cannot_become_a_size_comparison_or_other_platform(self):
        for args in [dict(candidate_only=False), dict(platform='macos'), dict(reuse_baseline='123')]:
            with self.subTest(args=args), self.assertRaises(ValueError):
                self.prepare({'origin': 'https://www.peakpass.dev'}, **args)

    def test_reject_ambiguous_or_credential_bearing_origin(self):
        for url in ['http://other.example', 'https://user:secret@other.example', 'https://other.example/api',
                    'https://other.example?token=x', 'https://other.example#x', 'https://controlled.example',
                    'https://bad host.example', 'https://other.example:invalid']:
            with self.subTest(url=url), self.assertRaises(ValueError):
                self.prepare({'origin': url})

    def test_expiry_fails_closed_at_each_variant_start(self):
        value = self.prepare({'origin': 'https://other.example', 'secondary_expires_unix': 4000})
        self.assertEqual(selected(value, 'secondary', 1000, 1800), 'https://other.example')
        with self.assertRaises(ValueError):
            selected(value, 'secondary', 3000, 1800)
        for expiry in [True, '4000', float('inf'), float('nan')]:
            with self.subTest(expiry=expiry), self.assertRaises(ValueError):
                self.prepare({'origin': 'https://other.example', 'secondary_expires_unix': expiry})

    def test_no_unknown_fields_or_duplicate_pair(self):
        for value in [{}, [], {'origin': 'https://other.example', 'source_ref': 'other-source'}]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.prepare(value)


if __name__ == '__main__':
    unittest.main()
