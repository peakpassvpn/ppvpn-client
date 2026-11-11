"""Verify reviewed draft inputs and preload exact Git objects for canonical archives.

These are restricted-history source inputs, not public candidate refs or path
dependencies. Cargo.toml and Cargo.lock keep their original Git source identity.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess

assert os.environ.get('GITHUB_ACTIONS') == 'true'
assert os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted'
repo = Path(os.environ['GITHUB_WORKSPACE'])
temp = Path(os.environ['RUNNER_TEMP'])
out = temp / 'apple-output/input'
out.mkdir(parents=True, exist_ok=False)
slug = os.environ['GITHUB_REPOSITORY']
release = json.loads(subprocess.check_output(['gh', 'api', f'repos/{slug}/releases/442305239']))
assert release['draft'] is True
assets = {item['name']: item for item in release['assets']}
expected = {
    'transport.json': '97b042338693602bca9d6774b972e753fd94035e2e9c99927110b5428b4b50ee',
    'normal.pack': 'd7b3464bc7bd3cff0f2278da8f54940a7b3f267145303bc72e805fc1d4a52016',
    'diagnostic.pack': '3962c7b69e881639aa8cf5672fb3926a8acd9675e13f2b10a1c3491db901226d',
    'sail-df9fdd3e.pack': 'c81454876b06e5be142f8a01a40c241c3eefaa83bb045987194a154552dc23de',
}

def download(name):
    asset = assets[name]
    assert asset['digest'] == 'sha256:' + expected[name]
    raw = subprocess.check_output(['gh', 'api', '-H', 'Accept: application/octet-stream', f"repos/{slug}/releases/assets/{asset['id']}"])
    assert len(raw) == asset['size']
    assert hashlib.sha256(raw).hexdigest() == expected[name]
    return raw

manifest = json.loads(download('transport.json'))
variant = os.environ['IOS256_VARIANT']
assert variant in ('normal', 'diagnostic')
client = manifest['variants'][variant]
sail = manifest['sail']
assert os.environ['SOURCE_REF'] == client['source']
assert os.environ['PPVPN_BUILD_NUMBER'] == client['build']
assert os.environ['CANDIDATE_ONLY'] == '1'
assert os.environ.get('PPVPN_INTERNAL_DIAGNOSTICS') == ('1' if client['internal_diagnostics'] else '0')
assert os.environ.get('PPVPN_INTERNAL_UDP_TARGET_DIAGNOSTICS') == '0'

def git(directory, *args):
    return subprocess.check_output(['git', '-C', str(directory), *args], text=True).strip()

base = client['public_base']
subprocess.run(['git', '-C', str(repo), 'fetch', '--no-tags', 'origin', base], check=True)
source = temp / 'ios256-source-client.git'
subprocess.run(['git', 'clone', '--bare', '--no-hardlinks', str(repo), str(source)], check=True)
assert git(source, 'cat-file', '-t', base + '^{tree}') == 'tree'
subprocess.run(['git', '-C', str(source), 'unpack-objects', '-q'], input=download(client['file']), check=True)
assert git(source, 'rev-parse', client['source'] + '^{tree}') == client['tree']
(source / 'shallow').write_text(client['source'] + '\n')
subprocess.run(['git', '-C', str(source), 'update-ref', 'refs/heads/frozen-ios256', client['source']], check=True)
subprocess.run(['git', '-C', str(source), 'symbolic-ref', 'HEAD', 'refs/heads/frozen-ios256'], check=True)
assert sail['commit'] in git(source, 'show', client['source'] + ':Cargo.toml')
assert '#' + sail['commit'] in git(source, 'show', client['source'] + ':Cargo.lock')

# Cargo1.98.1's existing normalized HTTPS source database identity, as used by
# the normal build. Seed objects directly; no Git URL rewrite or Cargo patch.
cargo_home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
db = cargo_home / 'git/db/sail-382837f928b67a90'
db.parent.mkdir(parents=True, exist_ok=True)
if not db.exists():
    subprocess.run(['git', 'init', '--bare', str(db)], check=True)
subprocess.run(['git', '-C', str(db), 'unpack-objects', '-q'], input=download(sail['file']), check=True)
assert git(db, 'rev-parse', sail['commit'] + '^{tree}') == sail['tree']
shallow = db / 'shallow'
roots = set(shallow.read_text().splitlines()) if shallow.exists() else set()
roots.add(sail['commit'])
shallow.write_text('\n'.join(sorted(roots)) + '\n')
subprocess.run(['git', '-C', str(db), 'update-ref', 'refs/commit/' + sail['commit'], sail['commit']], check=True)
subprocess.run(['git', '-C', str(db), 'fsck', '--connectivity-only', '--no-dangling'], check=True)
receipt = {
    'manifest': manifest, 'variant': variant, 'controller': os.environ['GITHUB_SHA'],
    'draft_release_id': release['id'], 'verified_payload_sha256': expected,
    'private_preload': {'client_git': str(source), 'sail_cargo_git_db': str(db)},
    'restrictions': 'Exact tips and trees, shallow candidate roots; not full history or publicly fetchable candidate refs. No old native libraries reused.',
    'cargo_source': 'Original HTTPS Git URL/revision and lockfile unchanged; no path patch or URL rewrite.',
}
proof = temp / 'ios256-canonical-clone-proof'
subprocess.run(['git', 'clone', '--no-hardlinks', str(source), str(proof)], check=True)
subprocess.run(['git', '-C', str(proof), 'checkout', '--detach', client['source']], check=True)
assert git(proof, 'rev-parse', 'HEAD^{tree}') == client['tree']
assert git(proof, 'status', '--porcelain') == ''
receipt['canonical_clone_proof'] = {'source': git(proof, 'rev-parse', 'HEAD'), 'tree': git(proof, 'rev-parse', 'HEAD^{tree}'), 'tracked_source_unchanged': True}
(out / 'source-preload-receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
with open(os.environ['GITHUB_ENV'], 'a') as env:
    env.write('IOS256_CLIENT_GIT=' + str(source) + '\n')
print('Verified source preload', variant, client['source'], sail['commit'])
