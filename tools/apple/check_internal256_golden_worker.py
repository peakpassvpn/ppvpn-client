"""Cross-language offline fixture gate on a hosted Apple worker only."""
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
inputs = Path(__file__).with_name('ios256-golden-input')
probe = Path(__file__).with_name('ios256-native-input') / 'InternalLoadProbe.swift'
manifest = json.loads((inputs / 'manifest.json').read_text())
assert manifest['source'] == '970b5d35343e4dfdb7b914464ca714cbdc65bd3d'
assert hashlib.sha256(probe.read_bytes()).hexdigest() == manifest['probe_sha256']
for name, expected in manifest['files'].items():
    assert not Path(name).is_absolute() and '..' not in Path(name).parts
    assert hashlib.sha256((inputs / name).read_bytes()).hexdigest() == expected
out = root / 'build/internal256-golden-worker'
out.mkdir(parents=True, exist_ok=False)
receipt = {'input': manifest, 'checks': [], 'scope': 'Fictional Python-produced golden and captured clock -> actual Swift contracts. No probe start, AppGroup, phone or network operation.'}

def run(name, command):
    started = time.monotonic()
    with (out / (name + '.stdout.txt')).open('w') as stdout, (out / (name + '.stderr.txt')).open('w') as stderr:
        result = subprocess.run(command, stdout=stdout, stderr=stderr, timeout=180)
    receipt['checks'].append({'name': name, 'command': command, 'exit': result.returncode, 'seconds': time.monotonic() - started})
    print(name, 'exit', result.returncode, flush=True)
    if result.returncode:
        raise RuntimeError(name + ' failed; raw logs retained')

try:
    xcodes = sorted(Path('/Applications').glob('Xcode_26*.app'))
    assert xcodes
    os.environ['DEVELOPER_DIR'] = str(xcodes[-1] / 'Contents/Developer')
    receipt['runner'] = {key: os.environ.get(key) for key in ['RUNNER_ENVIRONMENT', 'RUNNER_ARCH', 'ImageVersion', 'GITHUB_SHA', 'GITHUB_RUN_ID']}
    receipt['xcode'] = subprocess.check_output(['xcodebuild', '-version'], text=True)
    receipt['swift'] = subprocess.check_output(['xcrun', 'swiftc', '--version'], text=True)
    receipt['iphoneos_sdk'] = subprocess.check_output(['xcrun', '--sdk', 'iphoneos', '--show-sdk-version'], text=True).strip()
    binary = out / 'internal256-golden-checks'
    run('golden-compile', ['xcrun', 'swiftc', '-parse-as-library', '-warnings-as-errors', '-D', 'PPVPN_INTERNAL_DIAGNOSTICS', str(probe), str(inputs / 'native256-linked-contract.swift'), '-o', str(binary)])
    run('golden-execute', [str(binary), str(inputs / 'swift-golden')])
    assert 'PASS 15 linked256 golden checks' in (out / 'golden-execute.stdout.txt').read_text()
    receipt['status'] = 'PASS_HOSTED_PYTHON_TO_ACTUAL_SWIFT_GOLDEN15'
finally:
    receipt['raw_logs_sha256'] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in out.glob('*.txt')}
    (out / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
