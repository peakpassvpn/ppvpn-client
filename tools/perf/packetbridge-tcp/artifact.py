"""Authenticate this run's exact two native TCP host binaries and unchanged peer inputs before measurement."""
import hashlib,json,os,pathlib,subprocess,sys,zipfile
temp=pathlib.Path(os.environ['RUNNER_TEMP'])
def sha(p):return hashlib.file_digest(p.open('rb'),'sha256').hexdigest()
if sys.argv[1]=='export':
 p=temp/'tcp-build-output/BINARY-MANIFEST.json';m=json.loads(p.read_text());assert m['build_run']==os.environ['GITHUB_RUN_ID'] and len(m['arms'])==2
 with open(os.environ['GITHUB_OUTPUT'],'a') as f:f.write('manifest_sha256='+sha(p)+'\n')
else:
 repo=os.environ['GITHUB_REPOSITORY'];aid=os.environ['FRONT_ARTIFACT_ID'];assert aid.isdecimal();expected=os.environ['FRONT_ARTIFACT_SHA256'].removeprefix('sha256:')
 def api(path):return subprocess.check_output(['gh','api','repos/'+repo+'/'+path])
 meta=json.loads(api('actions/artifacts/'+aid));assert str(meta['workflow_run']['id'])==os.environ['GITHUB_RUN_ID'] and meta['workflow_run']['head_sha']==os.environ['GITHUB_SHA'] and meta['digest']=='sha256:'+expected
 jobs=json.loads(api('actions/runs/'+os.environ['GITHUB_RUN_ID']+'/jobs'));front=[j for j in jobs['jobs'] if j['name']=='Build exact PacketBridge TCP host arms'];assert len(front)==1 and front[0]['conclusion']=='success'
 raw=api('actions/artifacts/'+aid+'/zip');assert len(raw)==meta['size_in_bytes'] and hashlib.sha256(raw).hexdigest()==expected
 original=temp/'tcp-front.original.zip';original.write_bytes(raw);dst=temp/'tcp-front';dst.mkdir()
 with zipfile.ZipFile(original) as z:
  assert z.testzip() is None and len(z.namelist())==len(set(z.namelist()))
  assert all(not pathlib.Path(n).is_absolute() and '..' not in pathlib.Path(n).parts and (z.getinfo(n).external_attr>>16)&0o170000!=0o120000 for n in z.namelist());z.extractall(dst)
 bundle=dst/'tcp-build-output';manifest=bundle/'BINARY-MANIFEST.json';assert sha(manifest)==os.environ['FRONT_MANIFEST_SHA256'];m=json.loads(manifest.read_text())
 for arm,row in m['arms'].items():
  p=bundle/row['binary'];assert p.resolve().is_relative_to(bundle.resolve()) and p.stat().st_size==row['binary_bytes'] and sha(p)==row['binary_sha256'];p.chmod(0o755)
 for rel,row in m['files'].items():
  p=bundle/rel;assert p.resolve().is_relative_to(bundle.resolve()) and not p.is_symlink() and p.stat().st_size==row['bytes'] and sha(p)==row['sha256']
  if rel.startswith('bin/') or rel=='harness/netns.sh':p.chmod(0o755)
 with open(os.environ['GITHUB_ENV'],'a') as f:f.write('PB_TCP_BUNDLE='+str(bundle)+'\n')
 (temp/'tcp-front-authentication.json').write_text(json.dumps({'artifact':int(aid),'run':os.environ['GITHUB_RUN_ID'],'build_job':front[0]['id'],'ZIP_bytes':len(raw),'ZIP_sha256':expected,'CRC_pass':True,'manifest_sha256':sha(manifest),'both_binary_hashes_verified':True},indent=2)+'\n')
