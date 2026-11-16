"""Build exact two public-API example arms plus unchanged netgen/peer inputs."""
import hashlib,json,os,pathlib,shutil,subprocess,tarfile,time
from cargo_protocol import cargo_example_artifact
HERE=pathlib.Path(__file__).resolve().parent;pin=json.loads((HERE/'SOURCE.json').read_text());root=pathlib.Path(os.environ['PB_SOURCE']);out=pathlib.Path(os.environ['PB_OUTPUT']);out.mkdir(parents=True,exist_ok=False)
def sha(p):return hashlib.file_digest(p.open('rb'),'sha256').hexdigest()
def git(*args):return subprocess.check_output(['git',*args],cwd=root,text=True).strip()
assert git('rev-parse','HEAD')==pin['base'] and git('rev-parse','HEAD^{tree}')==pin['tree'] and not git('status','--porcelain','--untracked-files=all')
assert sha(root/'Cargo.lock')==pin['Cargo_lock_sha256'];assert len(os.sched_getaffinity(0))==4
cg=pathlib.Path('/sys/fs/cgroup')/pathlib.Path('/proc/self/cgroup').read_text().strip().split('::',1)[1].lstrip('/')
assert (cg/'memory.max').read_text().strip()=='8589934592' and (cg/'memory.swap.max').read_text().strip()=='0'
stages=[]
def run(name,args,cwd=root,env=None):
 start=time.monotonic()
 with (out/(name+'.stdout.raw')).open('wb')as stdout,(out/(name+'.stderr.raw')).open('wb')as stderr:p=subprocess.run(args,cwd=cwd,env=env,stdout=stdout,stderr=stderr)
 stages.append({'name':name,'argv':args,'exit':p.returncode,'seconds':time.monotonic()-start});(out/'stages.json').write_text(json.dumps(stages,indent=2)+'\n');assert p.returncode==0
 return out/(name+'.stdout.raw')
run('toolchain',['bash','-ec','rustc -vV; cargo --version; go version; uname -a'])
assert subprocess.check_output(['rustc','--version'],text=True).startswith('rustc 1.98.1 ')
assert 'go1.25.8' in subprocess.check_output(['go','version'],text=True)
for n,digest in pin['patches'].items():assert sha(HERE/n)==digest
assert sha(HERE/'example-only.patch')==pin['example_patch_sha256']
(out/'bin').mkdir();arms={};env=dict(os.environ,CARGO_BUILD_JOBS='1');example=root/pin['example_file'];assert not example.exists()
for arm,patch in [('old','old-bench.patch'),('new','candidate.patch')]:
 for name in [patch,'example-only.patch']:
  run(arm+'-'+name+'-check',['git','apply','--check',str(HERE/name)]);run(arm+'-'+name+'-apply',['git','apply',str(HERE/name)])
 assert git('diff','--name-only')=='sail/src/embed/tun.rs'
 assert git('ls-files','--others','--exclude-standard')==pin['example_file']
 assert sha(root/'sail/src/embed/tun.rs')==pin['patched_source'][arm] and sha(example)==pin['example_sha256']
 shutil.copy2(root/'sail/src/embed/tun.rs',out/(arm+'.tun.rs'));shutil.copy2(example,out/(arm+'.example.rs'))
 index=out/(arm+'-derived-source.index');tree_env=dict(os.environ,GIT_INDEX_FILE=str(index))
 run(arm+'-tree-base',['git','read-tree','HEAD'],env=tree_env);run(arm+'-tree-bind',['git','add','--','sail/src/embed/tun.rs',pin['example_file']],env=tree_env)
 derived_tree=run(arm+'-derived-tree',['git','write-tree'],env=tree_env).read_text().strip();assert len(derived_tree)==40;index.unlink()
 configs=[x for v in pin['cargo_profile_config']for x in ['--config',v]]
 raw=run(arm+'-compile',['cargo','build','-vv','--locked','--profile',pin['profile'],*configs,'-p','sail','--example','packet_bridge_tcp_bench','--features','inbound-tun','--message-format=json'],env=env)
 assert sha(root/'sail/src/embed/tun.rs')==pin['patched_source'][arm] and sha(example)==pin['example_sha256'] and git('diff','--name-only')=='sail/src/embed/tun.rs' and git('ls-files','--others','--exclude-standard')==pin['example_file']
 path,selection=cargo_example_artifact(raw.read_text(),root,stages[-1]['exit']);(out/(arm+'-artifact-selection.json')).write_text(json.dumps(selection,indent=2)+'\n')
 target=pathlib.Path(path);assert target.is_file() and not target.is_symlink() and target.resolve().is_relative_to(root/'target')
 binary=out/'bin'/('packetbridge-'+arm);shutil.copy2(target,binary);binary.chmod(0o755)
 arms[arm]={'derived_source_tree':derived_tree,'source_sha256':pin['patched_source'][arm],'patch_sha256':pin['patches'][patch],'example_sha256':pin['example_sha256'],'binary':str(binary.relative_to(out)),'binary_sha256':sha(binary),'binary_bytes':binary.stat().st_size}
 run(arm+'-restore',['git','restore','--source=HEAD','--','sail/src/embed/tun.rs']);example.unlink();assert not git('status','--porcelain','--untracked-files=all')
env=dict(os.environ,CGO_ENABLED='0');run('netgen',['go','build','-trimpath','-o',str(out/'bin/netgen'),'.'],cwd=root/'scripts/netem/netgen',env=env)
archive=pathlib.Path(os.environ['PB_SINGBOX_ARCHIVE']);assert sha(archive)=='a684484d7477d1437282ee411f4d131d0340aaad60a7868841ebd5d87dd8a0c6'
with tarfile.open(archive)as tf:binary=tf.extractfile('sing-box-1.14.2-linux-amd64/sing-box').read()
assert hashlib.sha256(binary).hexdigest()=='fc9c6e6ab345f045b16a0ed10d1ff28d68e8e56e7749fca30738d1406e98d7b8';(out/'bin/sing-box').write_bytes(binary);(out/'bin/sing-box').chmod(0o755)
(out/'harness').mkdir();base=HERE.parent/'f997-baseline/harness'
for n,digest in [('run.py','54aff15a8dbfc6eac6fe4c1ba7f153d449d5d16a0a1d40bda6ba2186ebb2aa20'),('netns.sh','09cd154e0e2ef2918584199e8a3d3ce2deb1bcaa0fb6b1e8ac0530966347b7ed')]:assert sha(base/n)==digest;shutil.copy2(base/n,out/'harness'/n)
(out/'harness/netns.sh').chmod(0o755)
assert not git('status','--porcelain','--untracked-files=all') and sha(root/'Cargo.lock')==pin['Cargo_lock_sha256']
manifest={'base':pin['base'],'tree':pin['tree'],'lock_sha256':pin['Cargo_lock_sha256'],'profile':pin['profile'],'profile_difference':pin['profile_difference'],'cargo_profile_config':pin['cargo_profile_config'],'cargo_jobs':1,'arms':arms,'build_run':os.environ['GITHUB_RUN_ID'],'source_pre_post_clean':True,'stages':stages,'Go_cache_environment_preserved':{k:os.environ.get(k)for k in ['GOCACHE','GOMODCACHE','GOTMPDIR','GOBIN']},'files':{str(p.relative_to(out)):{'sha256':sha(p),'bytes':p.stat().st_size}for folder in ['bin','harness']for p in(out/folder).iterdir()}}
(out/'BINARY-MANIFEST.json').write_text(json.dumps(manifest,indent=2)+'\n')
