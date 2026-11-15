"""Hosted-only exact-source Swift provider A/B, no App/Rust build or network test."""
import hashlib,json,os,pathlib,shutil,subprocess,time
assert os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted'
assert os.environ['GITHUB_REPOSITORY']=='peakpassvpn/ppvpn-client'
temp=pathlib.Path(os.environ['RUNNER_TEMP']);out=temp/'packet-bench-output';out.mkdir()
repo=os.environ['GITHUB_REPOSITORY'];controller=pathlib.Path(os.environ['GITHUB_WORKSPACE'])
expected={'bench-aca7d482-three-tips.pack':'1deb5b56780445c46050457831945fc27216775bf5fbf5013bbe344c7ef17679','bench-aca7d482-three-tips.json':'a89448fb229603cdcef9bfc3934ea2ba0c31905f6cdc497b3e4a263b571d9c5a'}
release=json.loads(subprocess.check_output(['gh','api',f'repos/{repo}/releases/442941010']))
assert release['draft'] is True
assets={a['name']:a for a in release['assets']}
def download(name):
    meta=assets[name];assert meta['digest']=='sha256:'+expected[name]
    raw=subprocess.check_output(['gh','api','-H','Accept: application/octet-stream',f"repos/{repo}/releases/assets/{meta['id']}"])
    assert len(raw)==meta['size'] and hashlib.sha256(raw).hexdigest()==expected[name]
    return raw
manifest=json.loads(download('bench-aca7d482-three-tips.json'))
assert manifest['refs']=={'bench':'aca7d4823ab255f658476fe91c308169373b60e0','candidate':'8336089829aaa344889c7400847b4512bbf100b9','baseline':'89fe2df881f665b9d4cd9cfebdcf5a7dabf3236c'}
bare=temp/'packet-bench.git';subprocess.run(['git','init','--bare',str(bare)],check=True)
for key in ['gc.auto','maintenance.auto']:subprocess.run(['git','-C',str(bare),'config',key,'0'],check=True)
subprocess.run(['git','-C',str(bare),'unpack-objects','-q'],input=download('bench-aca7d482-three-tips.pack'),check=True)
(bare/'shallow').write_text('\n'.join(manifest['shallow_roots'])+'\n')
for name,sha in manifest['refs'].items():subprocess.run(['git','-C',str(bare),'update-ref','refs/heads/'+name,sha],check=True)
subprocess.run(['git','-C',str(bare),'symbolic-ref','HEAD','refs/heads/bench'],check=True)
subprocess.run(['git','-C',str(bare),'fsck','--full','--strict'],check=True)
source=temp/'packet-bench-source';subprocess.run(['git','clone','--no-hardlinks',str(bare),str(source)],check=True)
def git(*args):return subprocess.check_output(['git',*args],cwd=source,text=True).strip()
assert git('rev-parse','HEAD')==manifest['refs']['bench'] and git('rev-parse','HEAD^{tree}')==manifest['bench_tree']
assert git('rev-list','--all','--count')=='3'
assert set(git('diff','--name-only',manifest['refs']['candidate'],manifest['refs']['bench']).splitlines())==set(manifest['test_files'])
for name,digest in {**manifest['test_files'],**manifest['production_files']}.items():assert hashlib.sha256((source/name).read_bytes()).hexdigest()==digest,name
(out/'source-provenance.json').write_text(json.dumps({'controller':os.environ['GITHUB_SHA'],'run':int(os.environ['GITHUB_RUN_ID']),'manifest':manifest,'production_all_eight_match833':True,'source_clean_pre':git('status','--porcelain')=='','scope':'Swift provider plus modeled64 queues only; native Rust bridge, App/NE, phone and physical throughput/RAM are not tested here'},indent=2)+'\n')
(out/'xcode-version.txt').write_bytes(subprocess.check_output(['xcodebuild','-version']))
(out/'swift-version.txt').write_bytes(subprocess.check_output(['swiftc','--version']))
env={k:v for k,v in os.environ.items() if k not in ['GH_TOKEN','GITHUB_TOKEN']}
started=time.monotonic()
with (out/'bench.stdout.raw').open('wb') as stdout,(out/'bench.stderr.raw').open('wb') as stderr:
    result=subprocess.run(['python3',str(controller/'tools/apple/audit_ios_packet_bench.py'),str(source),str(out/'command-audit')],cwd=source,env=env,stdout=stdout,stderr=stderr)
product=pathlib.Path('/tmp/ppvpn-ios-packet-ab-aca7d482')
if product.exists():shutil.copytree(product,out/'ab')
assert git('status','--porcelain')==''
(out/'execution-receipt.json').write_text(json.dumps({'source':manifest['refs']['bench'],'production_source':manifest['refs']['candidate'],'baseline':manifest['refs']['baseline'],'run':int(os.environ['GITHUB_RUN_ID']),'exit':result.returncode,'seconds':time.monotonic()-started,'source_clean_post':True,'device_or_network_action':False,'full_App_build':False,'Rust_native_modeled_not_linked':True},indent=2)+'\n')
if result.returncode:raise SystemExit(result.returncode)
rows=json.loads((out/'ab/RESULTS.json').read_text());assert len(rows['rows'])==4
print('Actual managed Swift provider A/B completed; hardware throughput and NE memory acceptance remain unproven')
