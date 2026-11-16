import hashlib,json,os,pathlib,subprocess
out=pathlib.Path(os.environ['RUNNER_TEMP'])/'abc-output';out.mkdir();repo=os.environ['GITHUB_REPOSITORY']
meta=json.loads(subprocess.check_output(['gh','api','repos/'+repo+'/releases/442941010']));assert meta['draft'];assets={a['name']:a for a in meta['assets']}
pins={'ab-cf2-833-89fe.pack':'5a60775006d7680c5ba20fb93e95287b59e14134d4fb6d14aecfd540edf8dc7a','ab-frozen-inputs.json':'dbacb2463367de95a31c4e81eda4961f1842c1a3e0c2c54fe05cb2e3d01140ad'}
def download(name):
 a=assets[name];assert a['digest']=='sha256:'+pins[name];b=subprocess.check_output(['gh','api','-H','Accept: application/octet-stream','repos/'+repo+'/releases/assets/'+str(a['id'])]);assert len(b)==a['size'] and hashlib.sha256(b).hexdigest()==pins[name];return b
m=json.loads(download('ab-frozen-inputs.json'));assert m['head']=='cf2d13d2a59553f9b6ed4b0cb5d9416c653e2d36'
bare=pathlib.Path(os.environ['RUNNER_TEMP'])/'abc-source.git';subprocess.run(['git','init','--bare',str(bare)],check=True);subprocess.run(['git','-C',str(bare),'unpack-objects','-q'],input=download('ab-cf2-833-89fe.pack'),check=True);(bare/'shallow').write_text('\n'.join(m['tips'])+'\n')
for i,tip in enumerate(m['tips']):subprocess.run(['git','-C',str(bare),'update-ref','refs/heads/'+('frozen' if i==0 else 'input'+str(i)),tip],check=True)
subprocess.run(['git','-C',str(bare),'symbolic-ref','HEAD','refs/heads/frozen'],check=True);subprocess.run(['git','-C',str(bare),'fsck','--strict','--no-reflogs'],check=True)
source=pathlib.Path(os.environ['RUNNER_TEMP'])/'abc-source';subprocess.run(['git','clone','--no-hardlinks',str(bare),str(source)],check=True)
for tip,tree in m['trees'].items():assert subprocess.check_output(['git','rev-parse',tip+'^{tree}'],cwd=source,text=True).strip()==tree
for rel,pin in m['source_files'].items():assert hashlib.sha256((source/rel).read_bytes()).hexdigest()==pin
assert hashlib.sha256((source/'Cargo.lock').read_bytes()).hexdigest()==m['cargo_lock_sha256']
(out/'frozen-source-inputs.json').write_text(json.dumps(m,indent=2)+'\n')
with open(os.environ['GITHUB_ENV'],'a') as f:f.write('ABC_SOURCE='+str(source)+'\n')

# The exact failed-run original, authenticated again by the source-owned reuse function.
original=subprocess.check_output(['gh','api','repos/'+repo+'/actions/artifacts/14195119284/zip'])
assert len(original)==87687473 and hashlib.sha256(original).hexdigest()=='7564c75a91a4fa12909ec8ad45f4ef7649f10b67bcbac9dc91ad11a631888963'
p=pathlib.Path(os.environ['RUNNER_TEMP'])/'legacy-original43798221841.zip';p.write_bytes(original)
with open(os.environ['GITHUB_ENV'],'a') as f:f.write('LEGACY_ORIGINAL_ZIP='+str(p)+'\n')
