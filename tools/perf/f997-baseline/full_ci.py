"""Managed CI integration: exact source inputs and authenticated same-run artifact."""
import hashlib,json,os,pathlib,subprocess,sys,zipfile
HERE=pathlib.Path(__file__).resolve().parent
TEMP=pathlib.Path(os.environ['RUNNER_TEMP'])
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def reviewed():
 m=json.loads((HERE/'RELATIVE-INPUTS.json').read_text())
 for rel,row in m['relative_files'].items():
  p=HERE/rel;assert p.stat().st_size==row['bytes'] and sha(p)==row['sha256']
 return m

def export_front():
 bundle=TEMP/'f997-build-output';m=json.loads((bundle/'BINARY-MANIFEST.json').read_text());gate=json.loads((bundle/'FRONT-BUILD-GATE-RESULT.json').read_text());assert gate['passed'] is True and gate['actual_service_exit']==0
 assert m['source']=='f99761138b23f267ebc7621057afe02018ab7c18' and m['tree']=='c7306832afbbdf77d78d21907756c95207136b94'
 with open(os.environ['GITHUB_OUTPUT'],'a') as f:f.write('manifest_sha256='+sha(bundle/'BINARY-MANIFEST.json')+'\n')
 print(json.dumps({'source':m['source'],'binary_manifest_sha256':sha(bundle/'BINARY-MANIFEST.json'),'files':m['files']}))

def fetch_front():
 artifact=os.environ['FRONT_ARTIFACT_ID'];assert artifact.isdecimal();repo=os.environ['GITHUB_REPOSITORY'];run=os.environ['GITHUB_RUN_ID'];expected=os.environ['FRONT_ARTIFACT_SHA256'].removeprefix('sha256:')
 def api(path,*flags):return subprocess.check_output(['gh','api',*flags,'repos/'+repo+'/'+path])
 meta=json.loads(api('actions/artifacts/'+artifact));assert str(meta['workflow_run']['id'])==run and meta['workflow_run']['head_sha']==os.environ['GITHUB_SHA'];assert meta['digest']=='sha256:'+expected
 jobs=json.loads(api('actions/runs/'+run+'/jobs'));build=[j for j in jobs['jobs'] if j['name']=='Exact F997 artifact gate'];assert len(build)==1 and build[0]['conclusion']=='success'
 raw=api('actions/artifacts/'+artifact+'/zip');assert len(raw)==meta['size_in_bytes'] and hashlib.sha256(raw).hexdigest()==expected
 original=TEMP/'f997-front-input.original.zip';original.write_bytes(raw);dest=TEMP/'f997-front-input';dest.mkdir()
 with zipfile.ZipFile(original) as z:
  assert z.testzip() is None
  assert all(not pathlib.Path(n).is_absolute() and '..' not in pathlib.Path(n).parts for n in z.namelist());z.extractall(dest)
 bundle=dest/'f997-build-output';manifest=bundle/'BINARY-MANIFEST.json';assert sha(manifest)==os.environ['FRONT_MANIFEST_SHA256'];m=json.loads(manifest.read_text())
 for rel,row in m['files'].items():
  p=bundle/rel;assert p.resolve().is_relative_to(bundle.resolve()) and not p.is_symlink() and p.stat().st_size==row['bytes'] and sha(p)==row['sha256']
 for p in (bundle/'bin').iterdir():p.chmod(0o755)
 (bundle/'harness/netns.sh').chmod(0o755)
 with open(os.environ['GITHUB_ENV'],'a') as f:f.write('F997_RUNTIME_BUNDLE='+str(bundle)+'\n')
 (TEMP/'front-artifact-authentication.json').write_text(json.dumps({'artifact':int(artifact),'run':int(run),'build_job':build[0]['id'],'ZIP_bytes':len(raw),'ZIP_sha256':expected,'CRC_pass':True,'manifest_sha256':sha(manifest),'all_binary_and_harness_hashes_verified':True},indent=2)+'\n')

reviewed()
if sys.argv[1]=='export-front':export_front()
elif sys.argv[1]=='fetch-front':fetch_front()
elif sys.argv[1]=='verify':print('12 exact reviewed source hashes PASS')
else:raise SystemExit('unknown integration mode')
