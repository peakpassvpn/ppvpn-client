"""Read back each instrumented APK's packaged JNI without changing build inputs."""
import hashlib,importlib.util,json,os,pathlib,re,shutil,subprocess,sys,zipfile
root=pathlib.Path(os.environ['SDK2A9_CLIENT']);out=pathlib.Path(os.environ['RUNNER_TEMP'])/'sdk2a9-output';phase=sys.argv[1]
assert phase in ['forwarding','smoke-tablet','smoke-phone']
original=json.loads((out/'runtime-native-reuse.json').read_text());expected={r['path']:r for r in original['native_libraries']}
for name,row in expected.items():
 _,abi,filename=name.split('/');p=root/'apps/flutter/android/app/src/main/jniLibs'/abi/filename
 with p.open('rb') as f:assert hashlib.file_digest(f,'sha256').hexdigest()==row['sha256']
paths=list((root/'apps/flutter/build/app/outputs').rglob('app-debug.apk'));assert paths
hashes={p:hashlib.file_digest(p.open('rb'),'sha256').hexdigest() for p in paths};assert len(set(hashes.values()))==1
apk=paths[0];rows=[]
spec=importlib.util.spec_from_file_location('verified_reuse',root/'tools/android/reuse_runtime_apk.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
with zipfile.ZipFile(apk) as z:
 assert len(z.namelist())==len(set(z.namelist())) and z.testzip() is None
 names=[n for n in z.namelist() if n.endswith(('/libppvpn_flutter.so','/libppvpn_tunnel.so'))]
 abis=sorted({n.split('/')[1] for n in names});assert 'x86_64' in abis and set(abis)<={'arm64-v8a','x86_64'}
 assert set(names)=={f'lib/{abi}/{name}.so' for abi in abis for name in ['libppvpn_flutter','libppvpn_tunnel']}
 for n in sorted(names):
  data=z.read(n);sha=hashlib.sha256(data).hexdigest();assert sha==expected[n]['sha256'] and len(data)==expected[n]['bytes'],'Packaged JNI differs from authenticated shipping input: '+n
  rows.append(dict(path=n,bytes=len(data),sha256=sha,identical_to_authenticated_shipping_jni=True))
 manifest=module.manifest(z.read('AndroidManifest.xml'));assert manifest['package']=='com.peakpassvpn.ppvpn'
sdk=pathlib.Path(os.environ.get('ANDROID_HOME') or os.environ['ANDROID_SDK_ROOT']);signers=list((sdk/'build-tools').glob('*/apksigner'));assert signers
signer=max(signers,key=lambda p:tuple(map(int,re.findall(r'\d+',p.parent.name))))
r=subprocess.run([str(signer),'verify','--print-certs',str(apk)],capture_output=True,text=True,check=True)
certs=sorted(set(s.lower() for s in re.findall(r'certificate SHA-256 digest: ([0-9a-fA-F]+)',r.stdout)));assert certs
saved=out/(phase+'-instrumented.apk');assert not saved.exists();shutil.copyfile(apk,saved)
with saved.open('rb') as f:assert hashlib.file_digest(f,'sha256').hexdigest()==hashes[apk]
(out/(phase+'-instrumented-apk.json')).write_text(json.dumps(dict(phase=phase,source=original['product_source'],original_run=original['original_run'],apk_path=str(apk),archived_apk=str(saved),apk_sha256=hashes[apk],apk_bytes=apk.stat().st_size,manifest=manifest,certificate_sha256=certs,native_libraries=rows,input_jni_unchanged=True,scope='Actual instrumented debug APK readback; available ABI JNI byte-identical to authenticated shipping inputs. Test APK version/signature are separate from shipping release metadata; no phone evidence.'),indent=2)+'\n')
print('Instrumented APK JNI verified:',phase,abis)
