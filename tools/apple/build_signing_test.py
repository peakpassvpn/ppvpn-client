"""Execute the actual generated-project edits, including the signed-only path."""
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest


class IOSSigningEdits(unittest.TestCase):
    def test_signed_runner_edits_preserve_exports_and_support_both_identity_kinds(self):
        source = (Path(__file__).resolve().parents[2] / "apps/flutter/scripts/ci-build.sh").read_text()
        blocks = re.findall(r"python3 - <<'PY'\n(.*?)\nPY", source, re.S)
        link = next(block for block in blocks if 'flags = ["$(inherited)", "-force_load"' in block)
        signing = next(block for block in blocks if '"CODE_SIGN_STYLE": "Manual"' in block)
        for identity in ("Apple Distribution", "Apple Development", "A" * 40):
            with self.subTest(identity=identity), tempfile.TemporaryDirectory() as work:
                project = Path(work) / "ios/Runner.xcodeproj/project.pbxproj"
                project.parent.mkdir(parents=True)
                project.write_text("PRODUCT_BUNDLE_IDENTIFIER = com.peakpassvpn.ppvpn;\n" * 3)
                env = {**os.environ, "NATIVE": "-framework Security -lc", "IOS_TEAM_ID": "TEAM",
                       "IOS_APP_PROFILE": "PROFILE", "IOS_CODE_SIGN_IDENTITY": identity}
                for block in (link, signing):
                    subprocess.run([sys.executable, "-c", block], cwd=work, env=env,
                                   capture_output=True, text=True, check=True)
                result = project.read_text()
                self.assertEqual(result.count('CODE_SIGN_STYLE = "Manual";'), 3)
                self.assertEqual(result.count(f'CODE_SIGN_IDENTITY = "{identity}";'), 3)
                self.assertEqual(result.count('PROVISIONING_PROFILE_SPECIFIER = "PROFILE";'), 3)
                self.assertEqual(result.count('EXPORTED_SYMBOLS_FILE = "$(PROJECT_DIR)/Runner/Bridge.exports";'), 3)
                self.assertEqual(result.count('STRIP_STYLE = "non-global";'), 3)
                self.assertEqual(result.count('DEAD_CODE_STRIPPING = YES;'), 3)


if __name__ == "__main__":
    unittest.main()
