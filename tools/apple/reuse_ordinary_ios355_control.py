"""Bind the verified ordinary128 control without recompiling its App."""
import hashlib,json,os,pathlib,subprocess,zipfile
repo='peakpassvpn/ppvpn-client';run_id=43297532225;job_id=131459731823;artifact_id=13982559822
controller='43c0aa531b367f13b0cfa203459e3d91df8706df'
source='40674d1866708c8e1a767be7592e6f4a8bca34b5'
digest='4cc62f267efc01e9db6181fe4235aee57447ba5845e3bfce8e5947268f1f7574'
assert os.environ['GITHUB_REPOSITORY']==repo and os.environ['SDK2A9_PLATFORM']=='ios'
assert os.environ['PPVPN_REUSE_ORDINARY_355']=='true'and os.environ['PPVPN_BUILD_NUMBER']=='355'
assert os.environ['PPVPN_PRIVATE_BUILD_NUMBER']=='357'
out=pathlib.Path(os.environ['RUNNER_TEMP'])/'sdk2a9-output'
current=json.loads((out/'source-provenance.json').read_bytes())['manifest']
assert current['ios256_native_inputs_frozen']is True
def api(path):return json.loads(subprocess.check_output(['gh','api',f'repos/{repo}/actions/'+path],timeout=60))
run=api(f'runs/{run_id}');job=api(f'jobs/{job_id}');meta=api(f'artifacts/{artifact_id}')
assert run['head_sha']==controller and run['conclusion']=='success'
assert job['run_id']==run_id and job['conclusion']=='success'
assert any(s['name']=='Build exact ordinary package'and s['conclusion']=='success'for s in job['steps'])
assert meta['workflow_run']['id']==run_id and meta['workflow_run']['head_sha']==controller
assert meta['name']=='sdk2a9-ios-ordinary'and not meta['expired']and meta['size_in_bytes']==79186835 and meta['digest']=='sha256:'+digest
archive=pathlib.Path(os.environ['RUNNER_TEMP'])/'ordinary355-control.zip'
try:
 with archive.open('xb')as f:subprocess.run(['gh','api',f'repos/{repo}/actions/artifacts/{artifact_id}/zip'],stdout=f,check=True,timeout=120)
 with archive.open('rb')as f:assert hashlib.file_digest(f,'sha256').hexdigest()==digest
 assert archive.stat().st_size==79186835
 with zipfile.ZipFile(archive)as z:
  assert z.testzip()is None
  original=json.loads(z.read('source-provenance.json'));receipt=json.loads(z.read('build-receipt.json'));proof=json.loads(z.read('candidate/build-provenance.json'))
  assert original['controller']==controller and original['manifest']['source']==receipt['source']==proof['source_commit']==source
  for key in ['sail','cargo_lock_sha256','product_version']:assert original['manifest'][key]==current[key]
  assert receipt['product_version']=='0.7.5'and receipt['build']=='355'and receipt['platform']=='ios'
  assert not receipt['internal_diagnostics']and not receipt['udp_target_diagnostics']and not receipt['private_policy_present']
  assert proof['export_contract_check_exit']==0 and proof['build_number']=='355'
  assert proof['backend_origin_sha256']==hashlib.sha256(os.environ['PPVPN_API_BASE_DEFAULT'].encode()).hexdigest()
  assert {r['bundle']for r in receipt['actual_product_versions']}=={'com.peakpassvpn.ppvpn','com.peakpassvpn.ppvpn.tunnel'}
  assert all(r['version']=='0.7.5'and r['build']=='355'for r in receipt['actual_product_versions'])
  (out/'ordinary355-original-build-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
  (out/'ordinary355-original-build-provenance.json').write_text(json.dumps(proof,indent=2)+'\n')
 result=dict(control_source=source,control_tree=original['manifest']['tree'],control_run=run_id,control_job=job_id,control_controller=controller,control_artifact=artifact_id,control_artifact_sha256=digest,requested_diagnostic_source=os.environ['SOURCE_REF'],actual_control_rebuilt=False,diagnostic_Main_and_NE_reuse=False,ordinary128_equivalence_actual_SDK_resolve_pending=True,scope='Verified existing ordinary128 control, different exact source retained honestly; new iOS256-only Core branch and private Native changes require actual357 compiler and settings validation')
 (out/'ordinary355-control-reuse.json').write_text(json.dumps(result,indent=2)+'\n')
finally:archive.unlink(missing_ok=True)
print('Authenticated actual ordinary355 control; no ordinary App rebuild or diagnostic binary reuse')
