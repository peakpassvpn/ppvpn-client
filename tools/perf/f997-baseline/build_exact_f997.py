"""Front artifact gate only. Execute inside reviewed 4CPU/8GiB/1800s service."""
import hashlib,json,os,pathlib,shutil,subprocess,time
SOURCE='f99761138b23f267ebc7621057afe02018ab7c18';TREE='c7306832afbbdf77d78d21907756c95207136b94'
root=pathlib.Path(os.environ['F997_SOURCE_CHECKOUT']).resolve();out=pathlib.Path(os.environ['F997_BINARY_OUTPUT']).resolve();out.mkdir(parents=True,exist_ok=False)
def git(*a):return subprocess.check_output(['git',*a],cwd=root,text=True).strip()
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
assert git('rev-parse','HEAD')==SOURCE and git('rev-parse','HEAD^{tree}')==TREE and not git('status','--porcelain','--untracked-files=all')
assert sha(root/'Cargo.lock')=='dfeaf3c14bcf23fb02157e2e103352eeb27a3c738fbc6124678f8e2d4b58c68a'
assert len(os.sched_getaffinity(0))==4
# The outer service enforces limits. Read cgroup v2 proof in THIS process.
cg=pathlib.Path('/sys/fs/cgroup')/pathlib.Path('/proc/self/cgroup').read_text().strip().split('::',1)[1].lstrip('/')
assert (cg/'memory.max').read_text().strip()=='8589934592'
assert (cg/'memory.swap.max').read_text().strip()=='0'
q,p=map(int,(cg/'cpu.max').read_text().split());assert q==4*p
stages=[]
def run(name,args,cwd=root,env=None):
 start=time.monotonic()
 with (out/(name+'.original.log')).open('wb') as f:r=subprocess.run(args,cwd=cwd,env=env,stdout=f,stderr=subprocess.STDOUT)
 stages.append({'name':name,'command':args,'exit':r.returncode,'seconds':time.monotonic()-start});(out/'actual-build-stages.json').write_text(json.dumps(stages,indent=2)+'\n');assert r.returncode==0
run('actual-toolchains',['bash','-ec','rustc -vV; cargo --version; go version; uname -a'])
assert subprocess.check_output(['rustc','--version'],text=True).startswith('rustc 1.98.1 ')
assert 'go1.25.8' in subprocess.check_output(['go','version'],text=True)
env=dict(os.environ,CARGO_BUILD_JOBS='2');run('exact-cli',['cargo','build','--locked','--profile','dist','-p','sail-cli'],env=env)
(out/'bin').mkdir()
metadata=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1'],cwd=root,text=True));target=pathlib.Path(metadata['target_directory'])
shutil.copy2(target/'dist/sail',out/'bin/sail')
env=dict(os.environ,CGO_ENABLED='0');run('exact-netgen',['go','build','-trimpath','-o',str(out/'bin/netgen'),'.'],cwd=root/'scripts/netem/netgen',env=env)
run('netgen-buildinfo',['go','version','-m',str(out/'bin/netgen')])
archive=pathlib.Path(os.environ['F997_SINGBOX_ARCHIVE']);assert sha(archive)=='a684484d7477d1437282ee411f4d131d0340aaad60a7868841ebd5d87dd8a0c6'
import tarfile
with tarfile.open(archive) as tf:
 binary=tf.extractfile('sing-box-1.14.2-linux-amd64/sing-box').read()
assert hashlib.sha256(binary).hexdigest()=='fc9c6e6ab345f045b16a0ed10d1ff28d68e8e56e7749fca30738d1406e98d7b8'
(out/'bin/sing-box').write_bytes(binary)
for f in (out/'bin').iterdir():f.chmod(0o755)
(out/'harness').mkdir();reviewed=pathlib.Path(os.environ['F997_REVIEWED_HARNESS'])
assert sha(reviewed)=='54aff15a8dbfc6eac6fe4c1ba7f153d449d5d16a0a1d40bda6ba2186ebb2aa20'
shutil.copy2(reviewed,out/'harness/run.py');shutil.copy2(root/'scripts/netem/netns.sh',out/'harness/netns.sh');(out/'harness/netns.sh').chmod(0o755)
assert sha(out/'harness/netns.sh')=='09cd154e0e2ef2918584199e8a3d3ce2deb1bcaa0fb6b1e8ac0530966347b7ed'
assert not git('status','--porcelain','--untracked-files=all')
resource={n:(cg/n).read_text() for n in ['memory.max','memory.swap.max','memory.events','memory.peak','cpu.max','cpu.stat','cpuset.cpus.effective']}
events=dict(line.split() for line in resource['memory.events'].splitlines());assert not any(int(events.get(k,'0')) for k in ['oom','oom_kill','oom_group_kill'])
provenance={'source':SOURCE,'tree':TREE,'run':os.environ['GITHUB_RUN_ID'],'job_label':os.environ.get('GITHUB_JOB'),'source_pre_post_clean':True,'source_Cargo_lock_sha256':sha(root/'Cargo.lock'),'stages':stages,'resource_readback':resource,'Go_cache_environment_preserved':{k:os.environ.get(k) for k in ['GOCACHE','GOMODCACHE','GOTMPDIR','GOBIN']},'logs':{q.name:sha(q) for q in out.glob('*.log')}}
receipt=out/'build-provenance.json';receipt.write_text(json.dumps(provenance,indent=2)+'\n')
manifest={'source':SOURCE,'tree':TREE,'sail_source':SOURCE,'netgen_source':SOURCE,'cargo_lock_sha256':sha(root/'Cargo.lock'),'build_provenance_actual_exit':0,'build_run':os.environ['GITHUB_RUN_ID'],'build_job':os.environ.get('GITHUB_JOB'),'build_provenance_sha256':sha(receipt),'netgen_sources':{n:sha(root/'scripts/netem/netgen'/n) for n in ['main.go','go.mod','cc_linux.go','cc_other.go']},'files':{str(q.relative_to(out)):{'sha256':sha(q),'bytes':q.stat().st_size} for sub in ['bin','harness'] for q in (out/sub).iterdir()}}
(out/'BINARY-MANIFEST.json').write_text(json.dumps(manifest,indent=2)+'\n')
