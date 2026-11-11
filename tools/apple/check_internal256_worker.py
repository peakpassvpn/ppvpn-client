"""Run exact reviewed Swift/header checks only on a hosted Apple worker."""
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import time

assert os.environ.get('GITHUB_ACTIONS') == 'true'
assert os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted'
assert platform.system() == 'Darwin'
root = Path(__file__).resolve().parents[2]
inputs = Path(__file__).with_name('ios256-native-input')
manifest = json.loads((inputs / 'manifest.json').read_text())
assert manifest['source'] == 'fe01164124e11abf85d66bf2f6d14bb72bc4c960'
out = root / 'build/internal256-native-worker'
out.mkdir(parents=True, exist_ok=False)
receipt = {'input': manifest, 'checks': [], 'actions': 'compile and offline pure checks; no App, tunnel, device or network action'}

def run(name, args, timeout=180):
    start = time.monotonic()
    with (out / (name + '.stdout.txt')).open('w') as stdout, (out / (name + '.stderr.txt')).open('w') as stderr:
        result = subprocess.run(args, cwd=inputs, stdout=stdout, stderr=stderr, timeout=timeout)
    receipt['checks'].append({'name': name, 'command': args, 'exit': result.returncode, 'seconds': time.monotonic() - start})
    print(name, 'exit', result.returncode, flush=True)
    if result.returncode:
        raise RuntimeError(name + ' failed; see raw worker logs')

try:
    for name, item in manifest['files'].items():
        assert Path(name).name == name
        assert hashlib.sha256((inputs / name).read_bytes()).hexdigest() == item['sha256']
    xcodes = sorted(Path('/Applications').glob('Xcode_26*.app'))
    assert xcodes, 'Xcode26 unavailable'
    os.environ['DEVELOPER_DIR'] = str(xcodes[-1] / 'Contents/Developer')
    receipt['runner'] = {k: os.environ.get(k) for k in ['RUNNER_ENVIRONMENT', 'RUNNER_OS', 'RUNNER_ARCH', 'ImageVersion', 'GITHUB_SHA', 'GITHUB_RUN_ID']}
    receipt['macos'] = subprocess.check_output(['sw_vers'], text=True)
    receipt['xcode'] = subprocess.check_output(['xcodebuild', '-version'], text=True)
    receipt['swift'] = subprocess.check_output(['xcrun', 'swiftc', '--version'], text=True)
    sdk = subprocess.check_output(['xcrun', '--sdk', 'iphoneos', '--show-sdk-path'], text=True).strip()
    receipt['iphoneos_sdk'] = subprocess.check_output(['xcrun', '--sdk', 'iphoneos', '--show-sdk-version'], text=True).strip()
    binary = str(out / 'internal256-native-checks')
    run('native-compile', ['xcrun', 'swiftc', '-parse-as-library', '-warnings-as-errors', '-D', 'PPVPN_INTERNAL_DIAGNOSTICS', 'InternalLoadProbe.swift', 'main.swift', '-o', binary])
    run('native-execute', [binary], timeout=30)
    assert 'PASS 85 native spec/DNS checks' in (out / 'native-execute.stdout.txt').read_text()
    module = out / 'PPVPNTunnel'
    module.mkdir()
    (module / 'ppvpn_tunnel.h').write_bytes((inputs / 'ppvpn_tunnel.h').read_bytes())
    (module / 'module.modulemap').write_text('module PPVPNTunnel { header "ppvpn_tunnel.h" export * }\n')
    swift = ['PacketTunnelProvider.swift', 'TunnelHost.swift', 'NetworkState.swift', 'InternalResourceSampler.swift', 'InternalLoadProbe.swift']
    command = ['xcrun', 'swiftc', '-typecheck', '-warnings-as-errors', '-sdk', sdk, '-target', 'arm64-apple-ios15.0', '-D', 'PPVPN_INTERNAL_DIAGNOSTICS', '-Xcc', '-DPPVPN_INTERNAL_DIAGNOSTICS', '-I', str(module)] + swift
    run('iphoneos-native-sdk', command)
    receipt['status'] = 'PASS_HOSTED_APPLE_NATIVE85_AND_FULL_HEADER_SDK_CHECK'
finally:
    receipt['raw_logs_sha256'] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in out.glob('*.txt')}
    (out / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
