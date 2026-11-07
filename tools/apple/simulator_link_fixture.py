#!/usr/bin/env python3
"""Opt-in Xcode architecture regression; tiny C/Swift fixtures, no product build/run."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    script = repo / 'apps/flutter/scripts/ci-build.sh'
    blocks = re.findall(r"python3 - <<'PY'\n(.*?)\nPY", script.read_text(), re.S)
    link_edit = next(block for block in blocks if 'flags = ["$(inherited)", "-force_load"' in block)
    exports = (repo / 'apps/flutter/runner/ios/Runner/Bridge.exports').read_text()
    required = {line for line in exports.splitlines() if line}
    with tempfile.TemporaryDirectory(prefix='ppvpn-sim-architecture-fixture-') as directory:
        work = Path(directory) / 'ios'
        work.mkdir()
        for name in ['rust', 'Runner', 'Shared', 'PacketTunnel']:
            (work / name).mkdir()
        (work / 'Runner/Bridge.exports').write_text(exports)
        (work / 'stub.c').write_text('\n'.join('void ' + name.removeprefix('_') + '(void) {}' for name in required))
        (work / 'main.c').write_text('int main(void) { return 0; }\n')
        sdk = subprocess.check_output(['xcrun', '--sdk', 'iphonesimulator', '--show-sdk-path'], text=True).strip()
        subprocess.run(['xcrun', 'clang', '-target', 'arm64-apple-ios15.0-simulator', '-isysroot', sdk,
                        '-c', str(work / 'stub.c'), '-o', str(work / 'stub.o')], check=True, capture_output=True)
        subprocess.run(['xcrun', 'ar', 'rcs', str(work / 'rust/libppvpn_flutter.a'), str(work / 'stub.o')],
                       check=True, capture_output=True)
        ruby = '''require 'xcodeproj'
p=Xcodeproj::Project.new(File.join(ARGV[0], 'Runner.xcodeproj'))
p.add_build_configuration('Profile', :release)
t=p.new_target(:application, 'Runner', :ios, '15.0', nil, :c)
t.add_build_configuration('Profile', :release) unless t.build_configurations.any?{|c| c.name=='Profile'}
t.add_file_references([p.main_group.new_reference('main.c')])
p.main_group.new_group('Runner', 'Runner')
p.build_configurations.each{|c| c.build_settings['ONLY_ACTIVE_ARCH']='YES'}
t.build_configurations.each do |c|
 c.build_settings.merge!('PRODUCT_BUNDLE_IDENTIFIER'=>'com.peakpassvpn.ppvpn',
 'GENERATE_INFOPLIST_FILE'=>'YES', 'CODE_SIGNING_ALLOWED'=>'NO', 'ENABLE_DEBUG_DYLIB'=>'NO',
 'ARCHS'=>'$(ARCHS_STANDARD)', 'EXCLUDED_ARCHS[sdk=iphonesimulator*]'=>'i386')
end
p.save
'''
        subprocess.run(['ruby', '-rxcodeproj', '-e', ruby, str(work)], check=True, capture_output=True)
        env = {**os.environ, 'NATIVE': '-framework Security -lc'}
        subprocess.run([sys.executable, '-c', link_edit], cwd=work.parent, env=env, check=True, capture_output=True)
        command = ['xcodebuild', '-project', str(work / 'Runner.xcodeproj'), '-target', 'Runner',
                   '-configuration', 'Debug', '-sdk', 'iphonesimulator', '-destination', 'generic/platform=iOS Simulator',
                   'SYMROOT=' + str(work / 'products'), 'OBJROOT=' + str(work / 'objects'), 'CODE_SIGNING_ALLOWED=NO', 'build']
        before = subprocess.run(command, capture_output=True, text=True)
        before_log = before.stdout + before.stderr
        if before.returncode == 0 or 'Undefined symbols for architecture x86_64' not in before_log:
            raise RuntimeError('fixture did not reproduce the unsupported simulator slice')
        if not all(name in before_log for name in required):
            raise RuntimeError('fixture failure did not contain the bridge export contract')
        privacy = plistlib.dumps({'NSPrivacyTracking': False, 'NSPrivacyAccessedAPITypes': []})
        for target in ['Runner', 'PacketTunnel']:
            (work / target / 'PrivacyInfo.xcprivacy').write_bytes(privacy)
        (work / 'PacketTunnel/Info.plist').write_bytes((repo / 'apps/flutter/runner/ios/PacketTunnel/Info.plist').read_bytes())
        (work / 'PacketTunnel/PacketTunnel.entitlements').write_bytes(plistlib.dumps({}))
        (work / 'PacketTunnel/Tunnel.exports').write_text('# No tunnel C exports in this fixture.\n')
        (work / 'PacketTunnel/PacketTunnelProvider.swift').write_text('import NetworkExtension\nclass PacketTunnelProvider: NEPacketTunnelProvider {}\n')
        (work / 'PacketTunnel/TunnelHost.swift').write_text('import Foundation\n')
        (work / 'Shared/NetworkState.swift').write_text('import Foundation\n')
        env = os.environ.copy()
        env.pop('IOS_TEAM_ID', None)
        env.pop('IOS_TUNNEL_PROFILE', None)
        env.update(PPVPN_TUNNEL_XCFRAMEWORK='', PPVPN_TUNNEL_LIBS='')
        subprocess.run(['ruby', str(repo / 'apps/flutter/scripts/ios-tunnel-target.rb'),
                        str(work / 'Runner.xcodeproj'), '0.0.0', '1'], env=env, check=True, capture_output=True)
        after = subprocess.run(command, capture_output=True, text=True)
        if after.returncode:
            raise RuntimeError((after.stdout + after.stderr)[-5000:])
        binary = work / 'products/Debug-iphonesimulator/Runner.app/Runner'
        arch = subprocess.check_output(['xcrun', 'lipo', '-archs', str(binary)], text=True).strip()
        symbols = subprocess.check_output(['nm', '-gU', str(binary)], text=True)
        actual = {line.split()[-1] for line in symbols.splitlines() if line.split()}
        if arch != 'arm64' or required - actual:
            raise RuntimeError('final fixture app architecture or exports differ')
        trie = subprocess.check_output(['xcrun', 'dyld_info', '-exports', str(binary)], text=True)
        dynamic = {line.split()[-1] for line in trie.splitlines() if re.match(r'\s+0x[0-9A-Fa-f]+\s', line)}
        if dynamic != required:
            raise RuntimeError('final fixture dynamic lookup contract differs')
        result = {'scope': 'synthetic architecture/link regression only; no product runtime or device acceptance',
                  'before': {'exit': before.returncode, 'undefined_architecture': 'x86_64', 'missing_contract_count': len(required)},
                  'after_actual_ruby_rewrite': {'exit': after.returncode, 'architecture': arch, 'required_exports_present': len(required)},
                  'xcode': subprocess.check_output(['xcodebuild', '-version'], text=True).strip(),
                  'sdk': subprocess.check_output(['xcrun', '--sdk', 'iphonesimulator', '--show-sdk-version'], text=True).strip(),
                  'installed_or_launched': False, 'fixture_cleaned': True}
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
