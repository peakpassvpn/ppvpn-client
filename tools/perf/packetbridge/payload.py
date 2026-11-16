"""Exact two-arm native build, or compiler-free execution of those test binaries."""
import hashlib,json,os,pathlib,re,shutil,subprocess,time
HERE=pathlib.Path(__file__).resolve().parent;pin=json.loads((HERE/'SOURCE.json').read_text());root=pathlib.Path(os.environ['PB_SOURCE']);out=pathlib.Path(os.environ['PB_OUTPUT']);out.mkdir(parents=True,exist_ok=False);phase=os.environ['PB_PHASE']
def sha(p):return hashlib.file_digest(p.open('rb'),'sha256').hexdigest()
def git(*args):return subprocess.check_output(['git',*args],cwd=root,text=True).strip()
assert len(os.sched_getaffinity(0))==4
cg=pathlib.Path('/sys/fs/cgroup')/pathlib.Path('/proc/self/cgroup').read_text().strip().split('::',1)[1].lstrip('/')
assert (cg/'memory.max').read_text().strip()==str(pin['build' if phase=='build' else 'measure']['memory'])
assert (cg/'memory.swap.max').read_text().strip()=='0'
quota,period=map(int,(cg/'cpu.max').read_text().split());assert quota==4*period
stages=[]
def run(name,args,cwd=root,env=None,measured=False):
 command=['/usr/bin/time','-v','-o',str(out/(name+'.time.original.txt')),*args] if measured else args
 start=time.monotonic()
 with (out/(name+'.stdout.raw')).open('wb') as stdout,(out/(name+'.stderr.raw')).open('wb') as stderr:r=subprocess.run(command,cwd=cwd,env=env,stdout=stdout,stderr=stderr)
 row={'name':name,'command':command,'exit':r.returncode,'wall_seconds':time.monotonic()-start,'measurement_includes_compilation':False if measured else None};stages.append(row);(out/'stages.json').write_text(json.dumps(stages,indent=2)+'\n');assert r.returncode==0
 return out/(name+'.stdout.raw')

def cargo_test_artifact(text,checkout,cargo_exit):
 assert type(cargo_exit) is int and cargo_exit==0,'Actual cargo exit must be zero'
 checkout=pathlib.Path(checkout)
 package='path+'+(checkout/'sail').as_uri()+'#0.24.0'
 expected_profile={'opt_level':'3','debuginfo':0,'debug_assertions':False,'overflow_checks':False,'test':True}
 # Cargo -vv interleaves bracket-prefixed build-script text with JSON records.
 # Keep the original stream; accept only that known non-JSON envelope.
 executables=[];finished=[];verbose_lines=0;json_records=0
 for number,line in enumerate(text.splitlines(),1):
  try:obj=json.loads(line)
  except json.JSONDecodeError:
   if re.fullmatch(r'\[[^\]\r\n]+\](?: .*)?',line):verbose_lines+=1;continue
   raise ValueError('Unrecognized Cargo stdout record at line '+str(number))
  assert isinstance(obj,dict);json_records+=1
  if obj.get('reason')=='build-finished':finished.append(obj.get('success'))
  assert not (obj.get('reason')=='compiler-message' and obj.get('message',{}).get('level')=='error'),'Cargo reported a compiler error'
  target=obj.get('target',{});profile=obj.get('profile',{})
  profile_ok=isinstance(profile,dict) and set(profile)==set(expected_profile) and all(type(profile[k]) is type(v) and profile[k]==v for k,v in expected_profile.items())
  if obj.get('reason')=='compiler-artifact' and obj.get('package_id')==package and obj.get('manifest_path')==str(checkout/'sail/Cargo.toml') and target.get('name')=='sail' and target.get('kind')==['lib'] and target.get('crate_types')==['lib'] and target.get('src_path')==str(checkout/'sail/src/lib.rs') and profile_ok and obj.get('executable'):
   executable=obj['executable'];assert isinstance(executable,str)
   path=pathlib.Path(executable)
   assert path.is_absolute() and path.parent==checkout/'target/dist-mobile/deps' and re.fullmatch(r'sail-[0-9a-f]+',path.name) and executable in obj.get('filenames',[]),'Unexpected native libtest executable path'
   executables.append(executable)
 assert len(finished)==1 and finished[0] is True,'Require exactly one successful Cargo build-finished record'
 assert len(executables)==1,'Require exactly one actual sail libtest executable'
 return executables[0],{'verbose_build_script_lines':verbose_lines,'JSON_records':json_records,'build_finished_success':True,'actual_cargo_exit':cargo_exit,'executable':executables[0]}

if phase=='build':
 assert git('rev-parse','HEAD')==pin['base'] and git('rev-parse','HEAD^{tree}')==pin['tree'] and not git('status','--porcelain','--untracked-files=all')
 assert sha(root/'Cargo.lock')==pin['Cargo_lock_sha256']
 for name,digest in pin['patches'].items():assert sha(HERE/name)==digest
 run('toolchain',['bash','-ec','rustc -vV; cargo --version; uname -a'])
 assert subprocess.check_output(['rustc','--version'],text=True).startswith('rustc 1.98.1 ')
 (out/'bin').mkdir();arms={};env=dict(os.environ,CARGO_BUILD_JOBS=str(pin['build']['cargo_jobs']))
 for arm,patch in [('old','old-bench.patch'),('new','candidate.patch')]:
  run(arm+'-apply-check',['git','apply','--check',str(HERE/patch)]);run(arm+'-apply',['git','apply',str(HERE/patch)])
  assert git('diff','--name-only')=='sail/src/embed/tun.rs';assert sha(root/'sail/src/embed/tun.rs')==pin['patched_source'][arm]
  shutil.copy2(root/'sail/src/embed/tun.rs',out/(arm+'.tun.rs'))
  config_args=[item for value in pin['cargo_profile_config'] for item in ['--config',value]]
  log=run(arm+'-compile',['cargo','test','-vv','--locked','--profile',pin['profile'],*config_args,'-p','sail','--lib','--no-run','--message-format=json'],env=env)
  executable,selection=cargo_test_artifact(log.read_text(),root,stages[-1]['exit'])
  (out/(arm+'-cargo-artifact-selection.json')).write_text(json.dumps(selection,indent=2)+'\n')
  target=pathlib.Path(executable);assert target.is_file() and not target.is_symlink() and target.resolve().is_relative_to((root/'target').resolve())
  binary=out/'bin'/('sail-'+arm+'-tests');shutil.copy2(target,binary);binary.chmod(0o755)
  arms[arm]={'source_sha256':pin['patched_source'][arm],'patch_sha256':pin['patches'][patch],'binary':str(binary.relative_to(out)),'binary_sha256':sha(binary),'binary_bytes':binary.stat().st_size,'profile':pin['profile'],'cargo_profile_config':pin['cargo_profile_config'],'cargo_jobs':pin['build']['cargo_jobs']}
  run(arm+'-restore',['git','restore','--source=HEAD','--','sail/src/embed/tun.rs']);assert not git('status','--porcelain','--untracked-files=all')
 manifest={'base':pin['base'],'tree':pin['tree'],'lock_sha256':sha(root/'Cargo.lock'),'profile':pin['profile'],'profile_difference':pin['profile_difference'],'cargo_profile_config':pin['cargo_profile_config'],'cargo_jobs':pin['build']['cargo_jobs'],'arms':arms,'build_run':os.environ['GITHUB_RUN_ID'],'source_pre_post_clean':True,'stages':stages}
 (out/'BINARY-MANIFEST.json').write_text(json.dumps(manifest,indent=2)+'\n')
else:
 manifest=json.loads((root/'BINARY-MANIFEST.json').read_text());assert manifest['base']==pin['base'] and manifest['profile']==pin['profile'] and manifest['cargo_profile_config']==pin['cargo_profile_config'] and manifest['cargo_jobs']==pin['build']['cargo_jobs'] and manifest['build_run']==os.environ['GITHUB_RUN_ID']
 for arm,row in manifest['arms'].items():assert sha(root/row['binary'])==row['binary_sha256']
 env=dict(os.environ,SAIL_PACKET_BRIDGE_BENCH_ROUNDS=str(pin['rounds']))
 ordinary=run('new-ordinary',[str(root/manifest['arms']['new']['binary']),'embed::tun::tests','--nocapture'],env=env,measured=True)
 assert 'test result: ok. 11 passed; 0 failed; 2 ignored;' in ordinary.read_text()
 for arm in ['old','new']:
  coop=run(arm+'-cooperative-budget-regression',[str(root/manifest['arms'][arm]['binary']),'embed::tun::tests::packet_bridge_runner_resumes_after_cooperative_budget_exhaustion','--exact','--nocapture'],env=env,measured=True)
  assert 'test result: ok. 1 passed; 0 failed; 0 ignored;' in coop.read_text()
 for arm in ['old','new']:
  for bench in ['packet_bridge_loopbench','packet_bridge_runner_loopbench']:
   run(arm+'-'+bench,[str(root/manifest['arms'][arm]['binary']),'embed::tun::tests::'+bench,'--exact','--ignored','--nocapture'],env=env,measured=True)
 (out/'MEASUREMENT-RESULT.json').write_text(json.dumps({'passed':True,'scope':pin['runtime'],'binary_manifest_sha256':sha(root/'BINARY-MANIFEST.json'),'stages':stages,'rounds':pin['rounds'],'capacity':pin['capacity'],'packet_bytes':pin['packet_bytes'],'compiler_invocations':0,'memory_caveat':'Queue cap64 unchanged; working receive batch may add up to4128705 payload bytes plus metadata/copy transients; sampled ledger excludes some received/pre-accounting memory. Per-process RSS is in original GNU time records; cgroup runtime peak excludes build.'},indent=2)+'\n')
(out/'actual-cgroup-readback.json').write_text(json.dumps({n:(cg/n).read_text() for n in ['memory.max','memory.swap.max','memory.events','memory.peak','cpu.max','cpu.stat','cpuset.cpus.effective']},indent=2)+'\n')
