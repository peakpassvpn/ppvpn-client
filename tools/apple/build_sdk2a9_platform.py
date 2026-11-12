"""Build normal exact source with existing platform recipes, preserving logs."""
import hashlib,json,os,pathlib,subprocess,tarfile,time,shutil
from product_version_checks import workspace_version,apple_versions,android_versions
root=pathlib.Path(os.environ['SDK2A9_CLIENT']);temp=pathlib.Path(os.environ['RUNNER_TEMP']);out=temp/'sdk2a9-output';platform=os.environ['SDK2A9_PLATFORM'];env=dict(os.environ)
assert env['PPVPN_INTERNAL_DIAGNOSTICS']==env['PPVPN_INTERNAL_UDP_TARGET_DIAGNOSTICS']=='0'
assert env.get('PPVPN_INTERNAL_IOS_CAP256_CANDIDATE','0')=='0'
assert not env.get('PPVPN_PRIVATE_UDP_POLICY_FILE') and not env.get('PPVPN_PRIVATE_UDP_POLICY_SHA256')
for key in ['IOS_TEAM_ID','IOS_APP_PROFILE','IOS_TUNNEL_PROFILE','IOS_EXPORT_OPTIONS','PPVPN_API_BASE','PPVPN_FLUTTER_HOME','PPVPN_FLUTTER_SAMPLE','PPVPN_FLUTTER_SYNTHETIC_TRAFFIC']:env.pop(key,None)
source_manifest=json.loads((out/'source-provenance.json').read_text())['manifest']
expected_lock=source_manifest['cargo_lock_sha256']
assert hashlib.sha256((root/'Cargo.lock').read_bytes()).hexdigest()==expected_lock
product_version=workspace_version(root,env)
start=time.monotonic()
stages=[]
def run(name,args,where=root,extra=None):
 e=env.copy();e.update(extra or {})
 before=time.monotonic()
 print(json.dumps(dict(stage=name,event='begin')),flush=True)
 with (out/(name+'.log')).open('wb') as log:r=subprocess.run(args,cwd=where,env=e,stdout=log,stderr=subprocess.STDOUT)
 stages.append(dict(name=name,command=args,exit=r.returncode,seconds=time.monotonic()-before))
 (out/'stage-results.json').write_text(json.dumps(stages,indent=2)+'\n')
 print(json.dumps(dict(stage=name,event='end',exit=r.returncode,seconds=stages[-1]['seconds'])),flush=True)
 if r.returncode:raise SystemExit(r.returncode)
triples={'ios':['aarch64-apple-ios'],'macos':['aarch64-apple-darwin'],'android':['aarch64-linux-android','x86_64-linux-android']}[platform]
packages=['ppvpn-flutter','ppvpn-tunnel-host'] if platform!='macos' else ['ppvpn-flutter','ppvpn-desktop','ppvpn-service']
for triple in triples:
 args=['cargo','tree','--locked','--target',triple,'-e','features,no-dev']
 for package in packages: args+=['-p',package]
 if platform=='macos':args+=['--features','ppvpn-desktop/macos-user-runtime']
 run('ordinary-features-'+triple,args)
 graph=(out/('ordinary-features-'+triple+'.log')).read_text()
 for feature in ['internal-diagnostics','internal-udp-target-diagnostics','internal-ios-cap256-candidate','fault-injection','testing','lab','macos-capture-acceptance']:
  assert 'feature "'+feature+'"' not in graph, 'unexpected ordinary feature'
if platform=='ios':
 run('ios-archive',['bash',str(pathlib.Path(os.environ['GITHUB_WORKSPACE'])/'tools/apple/archive_ios_variants.sh'),'primary'],extra={'GITHUB_WORKSPACE':env['SDK2A9_GIT']})
 shutil.copytree(temp/'apple-output'/'candidate',out/'candidate')
else:
 target=pathlib.Path(env['CARGO_TARGET_DIR']);target.mkdir(exist_ok=True)
 (root/'target').symlink_to(target,target_is_directory=True)
 with (root/'.git/info/exclude').open('a') as f:f.write('\n/target\n')
 if platform=='android':
  run('android-package',['bash','tools/android/build-size-artifact.sh',str(root),str(out/'candidate'),'all'])
 else:
  env.update(PPVPN_MACOS_SINGLE_RUNTIME='1',MACOS_ARCHS='arm64',PPVPN_APPLE_TARGETS='aarch64-apple-darwin')
  checkpoint=out/'macos-peer-worker-checks'
  receipt=json.loads((checkpoint/'worker3-receipt.json').read_text())
  assert receipt['head']==subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
  assert receipt['tree']==subprocess.check_output(['git','rev-parse','HEAD^{tree}'],cwd=root,text=True).strip()
  assert (receipt['passed'],receipt['failed'],receipt['ignored'])==(3,0,0)
  assert receipt['early_return_regression']==dict(selector='single_runtime::handshake_tests::worker_accepted_configuration_failure_recovers',passed=1,failed=0,ignored=0,actual_closed_peer_and_injection_separately_labeled=True,config_rejection_then_authenticated_recovery=True)
  assert len(receipt['selectors'])==3 and all((r['passed'],r['failed'],r['ignored'])==(1,0,0) for r in receipt['selectors'])
  expected_stages=['desktop-peer-compile','desktop-audit-and-signature','desktop-image-generation','desktop-peer-timing','broker-libtest-compile','broker-config-early-return','broker-ignored-selector-list','broker-ignored-1','broker-ignored-2','broker-ignored-3']
  results=json.loads((checkpoint/'results.json').read_text())
  assert [r['name'] for r in results]==expected_stages and all(r['exit']==0 for r in results)
  expected_selectors=['single_runtime::handshake_tests::worker_regression_mac_maintenance_blocks_accept','single_runtime::handshake_tests::worker_regression_mac_pretransaction_blocks_identity_and_grant','single_runtime::handshake_tests::worker_regression_mac_prior_transaction_queues_second_connection']
  assert [r['selector'] for r in receipt['selectors']]==expected_selectors
  assert (checkpoint/'source-pre.json').read_bytes()==(checkpoint/'source-post.json').read_bytes()
  assert hashlib.sha256(pathlib.Path(receipt['binary']).read_bytes()).hexdigest()==receipt['binary_sha256']
  run('flutter-build',['bash','apps/flutter/scripts/ci-build.sh'])
  run('helper-build',['bash','tools/desktop/build-service.sh','macos-arm64'])
  push_mode=subprocess.check_output(['python3','tools/apple/macos_push_package.py','plan','--enabled',env.get('PPVPN_APPLE_PUSH_ENABLED','0'),'--migration-ready',env.get('PPVPN_MACOS_APNS_MIGRATION_READY','0')],cwd=root,env=env,text=True).strip()
  assert push_mode in ['polling','apns']
  if push_mode=='polling':run('ffi-build',['bash','apps/macos/scripts/bootstrap.sh','--release'])
  app=root/'apps/flutter/build/macos/Build/Products/Release/PPVPN.app'
  run('macos-package',['bash','apps/flutter/scripts/package-macos.sh',str(app),'arm64'],extra={'SKIP_BOOTSTRAP':'1'})
  app=root/'apps/flutter/build/macos-package-arm64/PPVPN.app'
  with tarfile.open(out/'PPVPN.app.tar.gz','w:gz') as tf:tf.add(app,arcname='PPVPN.app')
  if (root/'dist/macos').is_dir():shutil.copytree(root/'dist/macos',out/'dist')
assert hashlib.sha256((root/'Cargo.lock').read_bytes()).hexdigest()==expected_lock,'Client lock changed during actual package build'
if platform=='ios':actual_versions=apple_versions(temp/'apple-candidate/apps/flutter/build/ios/archive/Runner.xcarchive/Products/Applications/Runner.app',product_version,env['PPVPN_BUILD_NUMBER'],ios=True)
elif platform=='macos':actual_versions=apple_versions(app,product_version,env['PPVPN_BUILD_NUMBER'])
else:actual_versions=android_versions(out/'candidate',product_version,env['PPVPN_BUILD_NUMBER'],env)
def version(args):
 try:return subprocess.check_output(args,text=True,stderr=subprocess.STDOUT).strip()
 except (OSError,subprocess.CalledProcessError):return None
files={}
for p in root.rglob('*'):
 if p.is_file() and not p.is_symlink() and (platform=='macos' and 'macos-package-arm64/PPVPN.app/' in p.as_posix()):
  files[str(p.relative_to(root))]={'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()}
receipt=dict(source=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),tree=subprocess.check_output(['git','rev-parse','HEAD^{tree}'],cwd=root,text=True).strip(),cargo_lock_sha256=hashlib.sha256((root/'Cargo.lock').read_bytes()).hexdigest(),platform=platform,product_version=product_version,actual_product_versions=actual_versions,build=env['PPVPN_BUILD_NUMBER'],internal_diagnostics=False,udp_target_diagnostics=False,private_policy_present=False,backend_origin_sha256=hashlib.sha256(env['PPVPN_API_BASE_DEFAULT'].encode()).hexdigest(),rust=version(['rustc','--version']),flutter=version(['flutter','--version','--machine']),xcode=version(['xcodebuild','-version']),sdk=version(['xcrun','--sdk','iphoneos' if platform=='ios' else 'macosx','--show-sdk-version']) if platform!='android' else None,seconds=time.monotonic()-start,files=files,logical_bundle_bytes=sum(v['bytes'] for v in files.values()) if files else None,ios_host_default_cap=128,mobile_local_proxy=False,capacity256_accepted=False,scope='Actual ordinary build; ad-hoc/debug measurement signatures; no install/device/VPN/publication/runtime acceptance')
(out/'build-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
print('Built ordinary',platform,receipt['source'],flush=True)
