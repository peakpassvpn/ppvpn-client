"""Hosted-only R2 export contract checks. No App/Rust/device or live IO run."""
import hashlib,io,json,os,pathlib,shutil,subprocess,tarfile,time
assert os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted'
assert os.environ['GITHUB_REPOSITORY']=='peakpassvpn/ppvpn-client'
controller=pathlib.Path(os.environ['GITHUB_WORKSPACE']);temp=pathlib.Path(os.environ['RUNNER_TEMP']);out=temp/'post-speed-export-r2-evidence';out.mkdir()
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
delivery_file=controller/'tools/apple/post_speed_export_r2_delivery.json'
assert h(delivery_file)=='fd4a02c02610f48a781b03477242493a91b27ae22d914b222d91ba7f28225209'
delivery=json.loads(delivery_file.read_text());repo=os.environ['GITHUB_REPOSITORY']
release=json.loads(subprocess.check_output(['gh','api',f'repos/{repo}/releases/442941010']));assert release['draft'] is True
assets={a['name']:a for a in release['assets']}
def download(name,digest):
    a=assets[name];assert a['digest']=='sha256:'+digest
    raw=subprocess.check_output(['gh','api','-H','Accept: application/octet-stream',f"repos/{repo}/releases/assets/{a['id']}"])
    assert len(raw)==a['size'] and hashlib.sha256(raw).hexdigest()==digest
    return raw
archive=download('export-only-r2-3e33c490.tar.gz',delivery['archive_sha256']);assert len(archive)==delivery['archive_bytes']
(out/'frozen-export-only-r2.tar.gz').write_bytes(archive)
package=temp/'export-only-r2';package.mkdir()
with tarfile.open(fileobj=io.BytesIO(archive)) as tf:
    members=tf.getmembers();assert all(not m.issym() and not m.islnk() and not m.isdev() and not pathlib.PurePosixPath(m.name).is_absolute() and '..' not in pathlib.PurePosixPath(m.name).parts for m in members)
    files={m.name:m for m in members if m.isfile()};assert set(files)==set(delivery['file_sha256'])
    for name,digest in delivery['file_sha256'].items():assert hashlib.sha256(tf.extractfile(files[name]).read()).hexdigest()==digest
    tf.extractall(package,filter='data')
shutil.copyfile(delivery_file,out/'DELIVERY-R2.json')
shutil.copyfile(package/'FREEZE-R2.json',out/'FREEZE-R2.json')
bare=temp/'source833.git';subprocess.run(['git','init','--bare',str(bare)],check=True)
for key in ['gc.auto','maintenance.auto']:subprocess.run(['git','-C',str(bare),'config',key,'0'],check=True)
pack=download('client-83360898.pack','68a3364d0e10a5515daec85dfe1a23cc535ca95b9555248b06663957f74093ea')
subprocess.run(['git','-C',str(bare),'unpack-objects','-q'],input=pack,check=True)
source_sha='8336089829aaa344889c7400847b4512bbf100b9';tree='4a40e51e640f15f2dca31fc769dba660efa71a92'
(bare/'shallow').write_text(source_sha+'\n');subprocess.run(['git','-C',str(bare),'update-ref','refs/heads/frozen',source_sha],check=True)
subprocess.run(['git','-C',str(bare),'symbolic-ref','HEAD','refs/heads/frozen'],check=True)
subprocess.run(['git','-C',str(bare),'fsck','--full','--strict'],check=True)
source=temp/'source833';subprocess.run(['git','clone','--no-hardlinks',str(bare),str(source)],check=True)
def git(*a):return subprocess.check_output(['git',*a],cwd=source,text=True).strip()
assert git('rev-parse','HEAD')==source_sha and git('rev-parse','HEAD^{tree}')==tree and git('rev-list','--all','--count')=='1'
assert git('status','--porcelain')==''
input_paths=['apps/flutter/runner/ios/Runner/CapacityDiagnosticSession.swift','apps/flutter/runner/ios/Runner/InternalLoadProbe.swift']
inputs={name:h(source/name) for name in input_paths}
(out/'input-provenance.json').write_text(json.dumps({'controller':os.environ['GITHUB_SHA'],'run':int(os.environ['GITHUB_RUN_ID']),'source':source_sha,'tree':tree,'source_pack_sha256':hashlib.sha256(pack).hexdigest(),'source_files':inputs,'delivery_sha256':h(delivery_file),'package_sha256':hashlib.sha256(archive).hexdigest(),'package_files':delivery['file_sha256'],'native_composite_expected':delivery['native_composite_sha256'],'managed_host_only':True},indent=2)+'\n')
(out/'toolchain.txt').write_bytes(subprocess.check_output(['xcodebuild','-version'])+subprocess.check_output(['swiftc','--version'])+subprocess.check_output(['python3','--version']))
env={k:v for k,v in os.environ.items() if k not in ['GH_TOKEN','GITHUB_TOKEN']};records=[]
def run(label,args):
    started=time.monotonic()
    with (out/(label+'.stdout.raw')).open('wb') as stdout,(out/(label+'.stderr.raw')).open('wb') as stderr:
        r=subprocess.run(args,cwd=package,env=env,stdout=stdout,stderr=stderr)
    records.append({'stage':label,'command':args,'exit':r.returncode,'seconds':time.monotonic()-started})
    (out/'stage-results.json').write_text(json.dumps(records,indent=2)+'\n')
    print(json.dumps(records[-1]),flush=True)
    if r.returncode:raise SystemExit(r.returncode)
prepared=temp/'ppvpn-export-only-native-r2-prepared';native=temp/'ppvpn-export-only-native-r2'
try:
    run('unit-host-tests',['python3','-B','-m','unittest','discover','-s',str(package),'-p','test_export_only.py','-v'])
    run('native-prepare',['python3','-B',str(package/'native_contract.py'),'--repo',str(source),'--output',str(prepared),'--prepare-only'])
    assert h(prepared/'main.swift')==delivery['native_composite_sha256']
    run('native-compile-run',['python3','-B',str(package/'native_contract.py'),'--repo',str(source),'--output',str(native)])
    assert h(native/'main.swift')==delivery['native_composite_sha256']
    receipt=json.loads((native/'receipt.json').read_text());assert receipt['compile_exit']==receipt['run_exit']==0 and receipt['prepared_only'] is False
finally:
    for name,folder in [('prepared',prepared),('native',native)]:
        if folder.exists():
            dest=out/name;dest.mkdir()
            for file in folder.iterdir():
                if file.is_file() and file.name!='native-contract':shutil.copyfile(file,dest/file.name)
            if (folder/'native-contract').exists():
                (dest/'compiled-executable.json').write_text(json.dumps({'sha256':h(folder/'native-contract'),'bytes':(folder/'native-contract').stat().st_size},indent=2)+'\n')
    assert git('status','--porcelain')=='' and all(h(source/name)==digest for name,digest in inputs.items())
    assert all(h(package/name)==digest for name,digest in delivery['file_sha256'].items())
    (out/'source-postcheck.json').write_text(json.dumps({'source_clean_post':True,'exact_source_hashes_unchanged':True,'exact_package_hashes_unchanged':True,'device_or_public_test_action':False,'full_App_Rust_AB_rebuild':False},indent=2)+'\n')
print('R2 contract unit and actual managed Swift compile/run succeeded; UIKit/Bundle are fixtures and no device/network test was performed')
