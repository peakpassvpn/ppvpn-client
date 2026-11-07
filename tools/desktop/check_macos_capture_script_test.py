"""Read-only checks for the native staging script's codesign CLI contract."""
import shlex
import subprocess
import sys
import unittest
from pathlib import Path


class SigningContract(unittest.TestCase):
    def test_script_passes_inline_requirement_without_weakening_policy(self):
        script = Path(__file__).with_name("macos-capture-acceptance.sh").read_text()
        command = next(line for line in script.splitlines()
                       if line.strip().startswith("codesign --verify"))
        arguments = shlex.split(command)
        requirement = arguments[arguments.index("-R") + 1]
        self.assertTrue(requirement.startswith("="))
        for constraint in ("anchor apple generic", "identifier", "certificate 1",
                           "1.2.840.113635.100.6.2.6", "certificate leaf",
                           "1.2.840.113635.100.6.1.13", "subject.OU"):
            self.assertIn(constraint, requirement)

    @unittest.skipUnless(sys.platform == "darwin", "requires system Apple signature")
    def test_actual_inline_parser_accepts_apple_and_rejects_wrong_identifier(self):
        def verify(requirement):
            return subprocess.run(
                ["/usr/bin/codesign", "--verify", "--strict", "-R", requirement,
                 "/usr/bin/true"], capture_output=True, text=True, timeout=15)
        self.assertEqual(verify("=anchor apple").returncode, 0)
        rejected = verify('=anchor apple and identifier "ppvpn.invalid.test-only"')
        self.assertNotEqual(rejected.returncode, 0)
        self.assertNotIn("invalid requirement specification", rejected.stderr)
        malformed = verify("anchor apple")
        self.assertNotEqual(malformed.returncode, 0)
        self.assertIn("invalid requirement specification", malformed.stderr)


if __name__ == "__main__":
    unittest.main()
