"""Authenticate exact full-build artifact, then reuse its shipping JNI in isolation."""
import hashlib,importlib.util,json,os,pathlib,re,subprocess,zipfile
root=pathlib.Path(os.environ['SDK2A9_CLIENT']);out=pathlib.Path(os.environ['RUNNER_TEMP'])/'sdk2a9-output'
pin=json.loads(os.environ['SDK2A9_RUNTIME_PIN']);slug=os.environ['GITHUB_REPOSITORY']
assert slug=='peakpassvpn/ppvpn-client'
assert pin['source']==os.environ['SOURCE_REF'] and pin['build']==os.environ['PPVPN_BUILD_NUMBER']
assert pin['controller']=='eb013c73146424822d31570acb304eee4d32d4a9'
assert pin['run']==43298583088 and pin['job']==131463365379
assert len(pin['artifact_sha256'])==64 and all(c in '0123456789abcdef' for c in pin['artifact_sha256'])
def api(path):return json.loads(subprocess.check_output(['gh','api',f'repos/{slug}/actions/'+path]))
run=api('runs/'+str(pin['run']));assert run['head_sha']==pin['controller']
job=api('jobs/'+str(pin['job']));assert job['run_id']==pin['run'] and job['status']=='completed' and job['conclusion']=='success'
assert job['name'].startswith('sdk2a9_platforms (android,')
assert any(s['name']=='Build exact ordinary package' and s['conclusion']=='success' for s in job['steps'])
meta=api('artifacts/'+str(pin['artifact']));assert meta['name']=='sdk2a9-android-ordinary' and not meta['expired']
assert meta['workflow_run']['id']==pin['run'] and meta['workflow_run']['head_sha']==pin['controller']
assert meta['digest']=='sha256:'+pin['artifact_sha256'] and 0<meta['size_in_bytes']<1024**3
work=pathlib.Path(os.environ['RUNNER_TEMP'])/'sdk2a9-native-reuse';work.mkdir(exist_ok=False)
container=work/'original.zip'
with container.open('xb') as f:
 result=subprocess.run(['gh','api',f'repos/{slug}/actions/artifacts/{pin["artifact"]}/zip'],stdout=f,stderr=subprocess.PIPE)
assert result.returncode==0,'Artifact download failed; raw credential diagnostics withheld'
with container.open('rb') as f:assert hashlib.file_digest(f,'sha256').hexdigest()==pin['artifact_sha256']
assert container.stat().st_size==meta['size_in_bytes']
with zipfile.ZipFile(container) as z:
 assert len(z.namelist())==len(set(z.namelist()))
 original=json.loads(z.read('source-provenance.json'));current=json.loads((out/'source-provenance.json').read_text())
 assert original['controller']==pin['controller'] and original['manifest']==current['manifest']
 receipt=json.loads(z.read('build-receipt.json'));provenance=json.loads(z.read('candidate/build-provenance.json'))
 for triple in ['aarch64-linux-android','x86_64-linux-android']:
  graph=z.read('ordinary-features-'+triple+'.log').decode()
  for feature in ['internal-diagnostics','internal-udp-target-diagnostics','internal-ios-cap256-candidate','fault-injection','testing','lab','macos-capture-acceptance']:
   assert 'feature \"'+feature+'\"' not in graph
 assert receipt['source']==pin['source'] and receipt['tree']==current['manifest']['tree']
 assert receipt['platform']=='android' and receipt['build']==pin['build']
 assert receipt['internal_diagnostics'] is False and receipt['udp_target_diagnostics'] is False and receipt['private_policy_present'] is False
 lock=hashlib.sha256((root/'Cargo.lock').read_bytes()).hexdigest();origin=os.environ['PPVPN_API_BASE_DEFAULT'];origin_sha=hashlib.sha256(origin.encode()).hexdigest()
 assert receipt['cargo_lock_sha256']==provenance['cargo_lock_sha256']==lock
 assert receipt['backend_origin_sha256']==provenance['backend_origin_sha256']==origin_sha
 assert provenance['source_commit']==pin['source'] and provenance['build_number']==pin['build']
 assert provenance['abis']==['arm64-v8a','x86_64'] and provenance['kind']=='all'
 apk=work/'app-release.apk';apk.write_bytes(z.read('candidate/app-release.apk'))
sdk=pathlib.Path(os.environ.get('ANDROID_HOME') or os.environ['ANDROID_SDK_ROOT'])
signers=list((sdk/'build-tools').glob('*/apksigner'));assert signers
signer=max(signers,key=lambda p:tuple(map(int,re.findall(r'\d+',p.parent.name))))
verified=subprocess.run([str(signer),'verify','--print-certs',str(apk)],capture_output=True,text=True,check=True)
certs=sorted(set(s.lower() for s in re.findall(r'certificate SHA-256 digest: ([0-9a-fA-F]+)',verified.stdout)))
assert certs and certs==provenance['verified_signing_certificate_sha256']['app-release.apk']
(out/'original-apk-signature.txt').write_text(verified.stdout)
# Reuse the exact source's existing ELF, manifest and compiled-origin verifier.
spec=importlib.util.spec_from_file_location('verified_reuse',root/'tools/android/reuse_runtime_apk.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
paths=[f'lib/{abi}/{name}.so' for abi in ['arm64-v8a','x86_64'] for name in ['libppvpn_flutter','libppvpn_tunnel']]
with zipfile.ZipFile(apk) as z:
 assert z.testzip() is None
 native=[dict(path=n,bytes=len(data),sha256=hashlib.sha256(data).hexdigest()) for n in paths for data in [z.read(n)]]
 observed=module.manifest(z.read('AndroidManifest.xml'))
assert observed==dict(package='com.peakpassvpn.ppvpn',versionCode=int(pin['build']),versionName=current['manifest']['product_version'])
apk_sha=hashlib.sha256(apk.read_bytes()).hexdigest()
info,payloads=module.validate_apk(apk,dict(apk_sha256=apk_sha,apk_bytes=apk.stat().st_size,native_libraries=native,manifest=observed),origin)
dest=root/'apps/flutter/android/app/src/main/jniLibs'
assert (root/'apps/flutter/android/app/src/main/AndroidManifest.xml').is_file()
for name,data in payloads.items():
 _,abi,filename=name.split('/');path=dest/abi/filename;assert not path.exists();path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(data)
report=dict(product_source=pin['source'],product_tree=current['manifest']['tree'],cargo_lock_sha256=lock,original_run=pin['run'],original_controller=pin['controller'],original_job=pin['job'],artifact=pin['artifact'],container_sha256=pin['artifact_sha256'],apk_sha256=apk_sha,apk_bytes=apk.stat().st_size,manifest=info,verified_certificate_sha256=certs,native_libraries=native,original_build_receipt=receipt,original_build_provenance=provenance,runtime_controller=os.environ['GITHUB_SHA'],android_native_cross_build_executed=False,scope='Authenticated original shipping JNI in an independently instrumented debug APK; runtime verdict pending; no production device action')
(out/'runtime-native-reuse.json').write_text(json.dumps(report,indent=2)+'\n')
print('Authenticated original full-build JNI copied; actual runtime verdict remains pending')
