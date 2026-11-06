#!/usr/bin/env python3
"""Opt-in real DMG/PKG regression using tiny synthetic binaries, never installed.

The resulting sizes are fixture sizes, not measurements of the client or core.
"""
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def main():
    if platform.system() != 'Darwin':
        raise SystemExit('Apple packaging tools are required')
    with tempfile.TemporaryDirectory(prefix='ppvpn-package-fixture-') as work:
        root = Path(work)
        for relative in ('apps/macos/scripts/package-app.sh', 'apps/macos/packaging/postinstall'):
            destination = root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / relative, destination)
        def run(*command, **kwargs):
            subprocess.run(command, check=True, capture_output=True, **kwargs)
        def info(path, executable, identifier):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(plistlib.dumps({'CFBundleExecutable': executable,
                'CFBundleIdentifier': identifier, 'CFBundlePackageType': 'APPL',
                'CFBundleShortVersionString': '0.0.0', 'CFBundleVersion': '1',
                'LSMinimumSystemVersion': '13.0'}))
        app = root / 'PPVPN.app'
        info(app / 'Contents/Info.plist', 'PPVPN', 'com.example.packagefixture')
        binaries = app / 'Contents/MacOS'
        binaries.mkdir()
        source = root / 'fixture.c'
        source.write_text('int main(void) { return 0; }\n')
        run('clang', '-mmacosx-version-min=13.0', str(source), '-o', str(binaries / 'PPVPN'))
        for name in ('service', 'service-install', 'service-uninstall'):
            shutil.copy2(binaries / 'PPVPN', binaries / ('ppvpn-' + name))
        framework = app / 'Contents/Frameworks/PPVPNClientFFI.framework'
        version = framework / 'Versions/A'
        version.mkdir(parents=True)
        (framework / 'Versions/Current').symlink_to('A')
        (framework / 'PPVPNClientFFI').symlink_to('Versions/Current/PPVPNClientFFI')
        (framework / 'Resources').symlink_to('Versions/Current/Resources')
        info(version / 'Resources/Info.plist', 'PPVPNClientFFI', 'com.example.packagefixture.ffi')
        library = version / 'PPVPNClientFFI'
        source.write_text('int fixture(void) { return 0; }\n')
        run('clang', '-mmacosx-version-min=13.0', '-dynamiclib', str(source), '-o', str(library),
            '-install_name', '@rpath/PPVPNClientFFI.framework/Versions/A/PPVPNClientFFI')
        agent = app / 'Contents/Library/LoginItems/PPVPN Agent.app'
        info(agent / 'Contents/Info.plist', 'PPVPN Agent', 'com.example.packagefixture.agent')
        (agent / 'Contents/MacOS').mkdir()
        source.write_text('extern int fixture(void); int main(void) { return fixture(); }\n')
        run('clang', '-mmacosx-version-min=13.0', str(source), str(library), '-o',
            str(agent / 'Contents/MacOS/PPVPN Agent'))
        # No release credentials, production flags or push registration.
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(('PPVPN_', 'SPARKLE_', 'ASC_', 'IOS_DIST_',
                                      'ANDROID_KEYSTORE_', 'ANDROID_KEY_'))}
        try:
            run('/bin/bash', str(root / 'apps/macos/scripts/package-app.sh'), str(app),
                platform.machine(), env=env)
        except subprocess.CalledProcessError as error:
            # No private credentials are inherited into this fixture command.
            raise SystemExit((error.stderr or b'').decode(errors='replace')[-5000:]) from error
        output = root / 'dist/macos'
        metadata = json.loads(next(output.glob('release-meta-*.json')).read_text())
        artifacts = {}
        for prefix, suffix in (('', '.dmg'), ('install_', '.pkg')):
            path = output / metadata[prefix + 'file']
            assert path.suffix == suffix
            data = path.read_bytes()
            sha = hashlib.sha256(data).hexdigest()
            assert len(data) == metadata[prefix + 'length']
            assert sha == metadata[prefix + 'sha256']
            artifacts[suffix] = {'bytes': len(data), 'sha256': sha}
        run('codesign', '--verify', '--deep', '--strict', str(app))
        print(json.dumps({'scope': 'synthetic packaging regression, not a client size comparison',
                          'installed': False, 'artifacts': artifacts, 'metadata_verified': True}))


if __name__ == '__main__':
    main()
