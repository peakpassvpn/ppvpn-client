"""Controlled359 managed compilation only; no credentials, launch or publication."""
import hashlib,json,os,pathlib,plistlib,shutil,subprocess,tarfile,time
root=pathlib.Path(os.environ['SDK2A9_CLIENT']);out=pathlib.Path(os.environ['RUNNER_TEMP'])/'auto359-output';ctl=pathlib.Path(os.environ['GITHUB_WORKSPACE']);env=dict(os.environ)
assert env['SOURCE_REF']=='04789dbccafd245fda71de26a575a7b31d15cfcf'
assert env['PPVPN_BUILD_NUMBER']=='359'
for k in list(env):
 if k.startswith(('PPVPN_NOTARY_','ASC_KEY','SPARKLE_PRIVATE')) or k in ['PPVPN_UPDATE_SITE','PPVPN_RELEASE_CHANNEL','PPVPN_UPDATE_FEED_URL','PPVPN_UPDATE_FEED','PPVPN_UPDATE_PUBLIC_KEY','PPVPN_SPARKLE_PUBLIC_KEY','FLUTTER_TARGET','PPVPN_SKIP_ANALYZE','PPVPN_FLUTTER_HOME','PPVPN_API_BASE']:env.pop(k,None)
env.update(ROOT_AUTO359_DISPATCH='ROOT_DISPATCH_AFTER_QA_aa399c_20261116',AUTO359_CHECKOUT=str(root),PPVPN_CONTROLLED_AUTO_ACCEPTANCE='1',PPVPN_ACCEPTANCE_BUILD_NUMBER='359',PPVPN_ACCEPTANCE_SOURCE_COMMIT=env['SOURCE_REF'],PPVPN_ACCEPTANCE_SOURCE_TREE='af5f27c52c39d5ffb571ea6cb9bf922f8ace7bd5')
target=pathlib.Path(env['CARGO_TARGET_DIR']);target.mkdir(exist_ok=True);(root/'target').symlink_to(target,target_is_directory=True)
with (root/'.git/info/exclude').open('a') as f:f.write('\n/target\n')
stages=[]
def run(name,args):
 start=time.monotonic();print(json.dumps({'stage':name,'event':'begin'}),flush=True)
 with (out/(name+'.log')).open('wb') as f:r=subprocess.run(args,cwd=root,env=env,stdout=f,stderr=subprocess.STDOUT)
 row={'name':name,'command':args,'exit':r.returncode,'seconds':time.monotonic()-start};stages.append(row);(out/'stage-results.json').write_text(json.dumps(stages,indent=2)+'\n');print(json.dumps(row),flush=True)
 if r.returncode:raise SystemExit(r.returncode)
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def capture():
 files={}
 for base in ['apps/flutter/lib/src/rust','apps/flutter/rust/src/frb_generated.rs','apps/flutter/pubspec.lock','apps/flutter/macos/Flutter/Release.xcconfig','apps/flutter/macos/Runner/Info.plist']:
  p=root/base
  for q in ([p] if p.is_file() else p.rglob('*') if p.exists() else []):
   if q.is_file() and not q.is_symlink():
    rel=q.relative_to(root);files[str(rel)]={'bytes':q.stat().st_size,'sha256':digest(q)};dest=out/'generated'/rel;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(q,dest)
 (out/'generated-input-output-hashes.json').write_text(json.dumps(files,indent=2)+'\n')
 for args,name in [(['git','diff','--binary'],'source-post.diff'),(['git','status','--porcelain','--untracked-files=all'],'source-post.status'),(['git','diff','--name-only'],'source-post.changed')]:
  (out/name).write_bytes(subprocess.check_output(args,cwd=root))
try:
 run('actual-toolchain',['bash','-c','rustc -vV; cargo --version; flutter --version --machine; dart --version; flutter_rust_bridge_codegen --version; cargo expand --version; xcodebuild -version; xcrun --sdk macosx --show-sdk-version; sw_vers; go version'])
 for tool in ['flutter_rust_bridge_codegen','cargo-expand']:
  p=pathlib.Path(shutil.which(tool));(out/(tool+'-binary-sha256.txt')).write_text(digest(p)+'\n')
 run('controlled-frb-dart-flutter-build',['bash',str(ctl/'tools/apple/auto359_compile.sh')])
 run('same-source-helper-build',['bash','tools/desktop/build-service.sh','macos-arm64'])
 run('same-source-legacy-ffi-build',['bash','apps/macos/scripts/bootstrap.sh','--release'])
 env['SKIP_BOOTSTRAP']='1'
 app=root/'apps/flutter/build/macos/Build/Products/Release/PPVPN.app'
 run('adhoc-package-no-notary',['bash','apps/flutter/scripts/package-macos.sh',str(app),'arm64'])
 app=root/'apps/flutter/build/macos-package-arm64/PPVPN.app'
 info=plistlib.loads((app/'Contents/Info.plist').read_bytes())
 assert info['CFBundleShortVersionString']=='0.7.5'
 assert str(info['CFBundleVersion']) in ['359','0.7.5.359']
 assert info['PPVPNControlledAutoAcceptance'] is True
 assert info['PPVPNMacRuntimeLayout']=='user-core-broker-v1'
 assert not info.get('PPVPNUpdateFeedURL') and not info.get('SUPublicEDKey')
 run('adhoc-strict-verification',['codesign','--verify','--deep','--strict','--verbose=4',str(app)])
 with tarfile.open(out/'PPVPN.app.tar.gz','w:gz') as tf:tf.add(app,arcname='PPVPN.app')
 shutil.copytree(root/'dist/macos',out/'dist')
 (out/'built-info.original.plist').write_bytes((app/'Contents/Info.plist').read_bytes())
 run('actual-app-UUIDs',['dwarfdump','--uuid',str(app/'Contents/MacOS/PPVPN'),str(app/'Contents/Frameworks/libppvpn_flutter.dylib')])
 files={str(p.relative_to(app)):{'bytes':p.stat().st_size,'sha256':digest(p)} for p in app.rglob('*') if p.is_file() and not p.is_symlink()}
 receipt={'scope':'ACTUAL_CONTROLLED_AUTO359_BUILD_ONLY_NOT_RUNTIME_ACCEPTANCE','source':env['SOURCE_REF'],'tree':env['PPVPN_ACCEPTANCE_SOURCE_TREE'],'build':359,'version':'0.7.5','fixture_id':env['PPVPN_ACCEPTANCE_FIXTURE_ID'],'compile_api_origin':env['PPVPN_API_BASE_DEFAULT'],'actual_bundle_version':info['CFBundleVersion'],'files':files,'signature':'CI ad-hoc; local Developer ID stage remains','notarized':False,'launched':False,'published':False,'stages':stages}
 (out/'build-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
finally:
 capture()
 assert digest(root/'Cargo.lock')=='a74cb9ffccd98f7365cc8531da73d627e4a57af28c3a8d957bec29a404863cc9'
