"""Fixed reviewed exact-tip source transport; no history/URL rewrite/path patch."""
import hashlib,json,os,pathlib,subprocess,tomllib
assert os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted'
temp=pathlib.Path(os.environ['RUNNER_TEMP']);out=temp/'sdk2a9-output';out.mkdir(exist_ok=True)
slug=os.environ['GITHUB_REPOSITORY'];release=json.loads(subprocess.check_output(['gh','api',f'repos/{slug}/releases/442941010']))
assert release['draft'] is True
assets={a['name']:a for a in release['assets']}
expected={'transport-89fe2df8.json':'c57936136e14456cfb6991f083747dc698f74f4412d0380de0d473d79d2fcc77','client-89fe2df8.pack':'f1745b2b98cad9515e7368b1c9793f763aef8d6420fdb766e0deb488a6fcd200'}
def download(name):
 a=assets[name];assert a['digest']=='sha256:'+expected[name]
 raw=subprocess.check_output(['gh','api','-H','Accept: application/octet-stream',f"repos/{slug}/releases/assets/{a['id']}"])
 assert len(raw)==a['size'] and hashlib.sha256(raw).hexdigest()==expected[name];return raw
manifest=json.loads(download('transport-89fe2df8.json'));assert manifest['source']==os.environ['SOURCE_REF'];assert manifest['build']==os.environ['PPVPN_BUILD_NUMBER']
# This controller must never dispatch a superseded SDK/Client lock by accident.
assert manifest['sail']=='f99761138b23f267ebc7621057afe02018ab7c18'
assert manifest['cargo_lock_sha256']=='a74cb9ffccd98f7365cc8531da73d627e4a57af28c3a8d957bec29a404863cc9'
assert os.environ['SDK2A9_PLATFORM'] in manifest['allowed_platforms']
assert manifest['internal_diagnostics'] is False and manifest['private_policy_present'] is False
assert manifest['ios_default_cap']==128 and manifest['mobile_local_proxy'] is False
bare=temp/'sdk2a9-source.git';subprocess.run(['git','init','--bare',str(bare)],check=True)
subprocess.run(['git','-C',str(bare),'unpack-objects','-q'],input=download('client-89fe2df8.pack'),check=True)
(bare/'shallow').write_text(manifest['source']+'\n')
subprocess.run(['git','-C',str(bare),'update-ref','refs/heads/frozen',manifest['source']],check=True)
subprocess.run(['git','-C',str(bare),'symbolic-ref','HEAD','refs/heads/frozen'],check=True)
checkout=temp/'sdk2a9-client';subprocess.run(['git','clone','--no-hardlinks',str(bare),str(checkout)],check=True)
def git(*args):return subprocess.check_output(['git','-C',str(checkout),*args],text=True).strip()
assert git('rev-parse','HEAD^{tree}')==manifest['tree']
assert git('status','--porcelain')==''
assert 'const IOS_MAX_CONNECTIONS: u32 = 128;' in (checkout/'crates/tunnel-host/src/host.rs').read_text()
assert hashlib.sha256((checkout/'Cargo.lock').read_bytes()).hexdigest()==manifest['cargo_lock_sha256']
assert tomllib.loads((checkout/'Cargo.toml').read_text())['workspace']['package']['version']==manifest['product_version']
assert manifest['sail'] in (checkout/'Cargo.toml').read_text()
assert '#'+manifest['sail'] in (checkout/'Cargo.lock').read_text()
assert not os.environ.get('PPVPN_PRIVATE_UDP_POLICY_FILE') and not os.environ.get('PPVPN_PRIVATE_UDP_POLICY_SHA256')
(out/'source-provenance.json').write_text(json.dumps(dict(manifest=manifest,controller=os.environ['GITHUB_SHA'],payload_sha256=expected,scope='Exact tip/tree only; parent history omitted; ordinary default-off source; no native reuse'),indent=2)+'\n')
with open(os.environ['GITHUB_ENV'],'a') as e:e.write(f'SDK2A9_CLIENT={checkout}\nSDK2A9_GIT={bare}\n')
