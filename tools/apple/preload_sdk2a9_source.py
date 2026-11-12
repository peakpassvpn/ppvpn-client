"""Fixed reviewed exact-tip source transport; no history/URL rewrite/path patch."""
import hashlib,json,os,pathlib,subprocess
assert os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted'
temp=pathlib.Path(os.environ['RUNNER_TEMP']);out=temp/'sdk2a9-output';out.mkdir(exist_ok=True)
slug=os.environ['GITHUB_REPOSITORY'];release=json.loads(subprocess.check_output(['gh','api',f'repos/{slug}/releases/442941010']))
assert release['draft'] is True
assets={a['name']:a for a in release['assets']}
expected={'transport.json':'0afd350651e3fbe593802051267479273acf1538da17265433da1e7a2ba3001e','client.pack':'18d3c0c4c247eb63c34c5784554dd20d4d5025cc94edd636aa4e1c0051575ee4'}
def download(name):
 a=assets[name];assert a['digest']=='sha256:'+expected[name]
 raw=subprocess.check_output(['gh','api','-H','Accept: application/octet-stream',f"repos/{slug}/releases/assets/{a['id']}"])
 assert len(raw)==a['size'] and hashlib.sha256(raw).hexdigest()==expected[name];return raw
manifest=json.loads(download('transport.json'));assert manifest['source']==os.environ['SOURCE_REF'];assert manifest['build']==os.environ['PPVPN_BUILD_NUMBER']
assert manifest['internal_diagnostics'] is False and manifest['private_policy_present'] is False
bare=temp/'sdk2a9-source.git';subprocess.run(['git','init','--bare',str(bare)],check=True)
subprocess.run(['git','-C',str(bare),'unpack-objects','-q'],input=download('client.pack'),check=True)
(bare/'shallow').write_text(manifest['source']+'\n')
subprocess.run(['git','-C',str(bare),'update-ref','refs/heads/frozen',manifest['source']],check=True)
subprocess.run(['git','-C',str(bare),'symbolic-ref','HEAD','refs/heads/frozen'],check=True)
checkout=temp/'sdk2a9-client';subprocess.run(['git','clone','--no-hardlinks',str(bare),str(checkout)],check=True)
def git(*args):return subprocess.check_output(['git','-C',str(checkout),*args],text=True).strip()
assert git('rev-parse','HEAD^{tree}')==manifest['tree']
assert git('status','--porcelain')==''
assert hashlib.sha256((checkout/'Cargo.lock').read_bytes()).hexdigest()==manifest['cargo_lock_sha256']
assert manifest['sail'] in (checkout/'Cargo.toml').read_text()
assert '#'+manifest['sail'] in (checkout/'Cargo.lock').read_text()
assert not os.environ.get('PPVPN_PRIVATE_UDP_POLICY_FILE') and not os.environ.get('PPVPN_PRIVATE_UDP_POLICY_SHA256')
(out/'source-provenance.json').write_text(json.dumps(dict(manifest=manifest,controller=os.environ['GITHUB_SHA'],payload_sha256=expected,scope='Exact tip/tree only; parent history omitted; ordinary default-off source; no native reuse'),indent=2)+'\n')
with open(os.environ['GITHUB_ENV'],'a') as e:e.write(f'SDK2A9_CLIENT={checkout}\nSDK2A9_GIT={bare}\n')
