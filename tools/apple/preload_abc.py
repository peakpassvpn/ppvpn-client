import hashlib,json,os,pathlib,subprocess
out=pathlib.Path(os.environ['RUNNER_TEMP'])/'abc-output';out.mkdir();repo=os.environ['GITHUB_REPOSITORY']
meta=json.loads(subprocess.check_output(['gh','api','repos/'+repo+'/releases/442941010']));assert meta['draft'];assets={a['name']:a for a in meta['assets']}
pins={'abc-a4e54-833-89fe.pack':'237cfbe266552ed7d86ed43a66cf38941976d594c81b443cd3898fe45eeed87c','abc-frozen-inputs.json':'4b129549a8c5cb35319cdaeea0489142e8126b89e252d3b12fd3ae2d92829d9b'}
def download(name):
 a=assets[name];assert a['digest']=='sha256:'+pins[name];b=subprocess.check_output(['gh','api','-H','Accept: application/octet-stream','repos/'+repo+'/releases/assets/'+str(a['id'])]);assert len(b)==a['size'] and hashlib.sha256(b).hexdigest()==pins[name];return b
m=json.loads(download('abc-frozen-inputs.json'));assert m['head']=='a4e54b4ba11bd1ded947f4d6cb0b02d7c1f2fece'
bare=pathlib.Path(os.environ['RUNNER_TEMP'])/'abc-source.git';subprocess.run(['git','init','--bare',str(bare)],check=True);subprocess.run(['git','-C',str(bare),'unpack-objects','-q'],input=download('abc-a4e54-833-89fe.pack'),check=True);(bare/'shallow').write_text('\n'.join(m['tips'])+'\n')
for i,tip in enumerate(m['tips']):subprocess.run(['git','-C',str(bare),'update-ref','refs/heads/'+('frozen' if i==0 else 'input'+str(i)),tip],check=True)
subprocess.run(['git','-C',str(bare),'symbolic-ref','HEAD','refs/heads/frozen'],check=True);subprocess.run(['git','-C',str(bare),'fsck','--strict','--no-reflogs'],check=True)
source=pathlib.Path(os.environ['RUNNER_TEMP'])/'abc-source';subprocess.run(['git','clone','--no-hardlinks',str(bare),str(source)],check=True)
for tip,tree in m['trees'].items():assert subprocess.check_output(['git','rev-parse',tip+'^{tree}'],cwd=source,text=True).strip()==tree
for rel,pin in m['source_files'].items():assert hashlib.sha256((source/rel).read_bytes()).hexdigest()==pin
assert hashlib.sha256((source/'Cargo.lock').read_bytes()).hexdigest()==m['cargo_lock_sha256']
(out/'frozen-source-inputs.json').write_text(json.dumps(m,indent=2)+'\n')
with open(os.environ['GITHUB_ENV'],'a') as f:f.write('ABC_SOURCE='+str(source)+'\n')
