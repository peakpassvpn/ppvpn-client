"""Revalidate completed lifecycle XML after an observer-only CI failure."""
import hashlib,io,json,os,pathlib,subprocess,xml.etree.ElementTree as ET,zipfile
repo='peakpassvpn/ppvpn-client'
run_id=43306465221;job_id=131490962045;artifact_id=13983844260
controller='4da5145ce16b65a2d84e4ccba7b6622966c61f88'
digest='04839fc9f0ed270a866ab86ba0e858558324ba5804a2fe3813f2c63be1f56e6e'
source='40674d1866708c8e1a767be7592e6f4a8bca34b5'
assert os.environ['GITHUB_REPOSITORY']==repo and os.environ['SOURCE_REF']==source
def gh(path,binary=False):
 raw=subprocess.check_output(['gh','api',*(['-H','Accept: application/octet-stream']if binary else[]),f'repos/{repo}/actions/'+path],timeout=60)
 return raw if binary else json.loads(raw)
run=gh(f'runs/{run_id}');job=gh(f'jobs/{job_id}');meta=gh(f'artifacts/{artifact_id}')
assert run['head_sha']==controller and run['conclusion']=='failure'
assert job['run_id']==run_id and job['conclusion']=='failure'
assert any(s['name']=='Prepare frozen runner and authenticate shipping JNI'and s['conclusion']=='success'for s in job['steps'])
assert any(s['name']=='Production service lifecycle with non-native host'and s['conclusion']=='failure'for s in job['steps'])
assert meta['name']=='sdk2a9-android-runtime'and not meta['expired']and meta['workflow_run']['id']==run_id and meta['workflow_run']['head_sha']==controller
assert meta['digest']=='sha256:'+digest and meta['size_in_bytes']==9114
raw=gh(f'artifacts/{artifact_id}/zip',binary=True)
assert len(raw)==9114 and hashlib.sha256(raw).hexdigest()==digest
out=pathlib.Path(os.environ['RUNNER_TEMP'])/'sdk2a9-output'
current=json.loads((out/'source-provenance.json').read_bytes())
with zipfile.ZipFile(io.BytesIO(raw))as z:
 assert z.testzip()is None and len(z.namelist())==len(set(z.namelist()))
 original=json.loads(z.read('sdk2a9-output/source-provenance.json'))
 assert original['controller']==controller and original['manifest']==current['manifest']and original['manifest']['source']==source
 reuse=json.loads(z.read('sdk2a9-output/runtime-native-reuse.json'))
 assert reuse['runtime_controller']==controller and reuse['product_source']==source
 log=z.read('sdk2a9-output/android-lifecycle.log');assert b'BUILD SUCCESSFUL'in log
 paths=[n for n in z.namelist()if n.endswith('/TEST-com.peakpassvpn.ppvpn.TunnelServiceTest.xml')];assert len(paths)==1
 xml=z.read(paths[0]);assert len(xml)<200000
 root=ET.fromstring(xml);assert root.tag=='testsuite'
 assert int(root.attrib['tests'])==10 and all(int(root.attrib.get(k,0))==0 for k in ['failures','errors','skipped'])
 cases=root.findall('testcase');assert len(cases)==10 and all(not list(c)for c in cases)
 (out/'prior-lifecycle-actual.xml').write_bytes(xml)
 (out/'prior-lifecycle-actual.log').write_bytes(log)
 receipt=dict(tests=10,failures=0,errors=0,skipped=0,original_run=run_id,original_job=job_id,original_controller=controller,original_artifact=artifact_id,original_artifact_sha256=digest,source=source,tree=current['manifest']['tree'],suite=dict(file=paths[0],**root.attrib),testcases=[c.attrib for c in cases],actual_tests_rerun=False,original_CI_failure_preserved=True,scope='Actual prior Robolectric production TunnelService with non-native TunnelHost; corrected report conversion only, not JNI or emulator/phone verdict')
 (out/'android-lifecycle-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
print('Prior exact-source actual lifecycle XML revalidated:10pass; no tests rerun; emulator verdict still pending')
