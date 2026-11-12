"""Real SDK settings validation after a strictly test-only compilation fix."""
import hashlib,json,os,pathlib,re,subprocess,time
root=pathlib.Path(os.environ['SDK2A9_CLIENT']);out=pathlib.Path(os.environ['RUNNER_TEMP'])/'sdk2a9-output'
env=dict(os.environ)
for key in list(env):
 if key.startswith(('SCCACHE_','CCACHE_','PPVPN_PRIVATE_UDP_'))or key in ['PPVPN_IOS256_PRIVATE_BUNDLE']:env.pop(key,None)
env.update(RUSTC_WRAPPER='',RUSTC_WORKSPACE_WRAPPER='')
source=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
assert source=='ddaef5de7d22817743b95be02f463a50f3f69fb1'
reference=json.loads(pathlib.Path(__file__).with_name('memory357-reference89-inputs.json').read_bytes())
files=reference['files_sha256'];current={name:hashlib.sha256((root/name).read_bytes()).hexdigest()for name in files}
assert set(subprocess.check_output(['git','ls-files'],cwd=root,text=True).splitlines())==set(files)
assert [name for name in files if current[name]!=files[name]]==['crates/core/src/engine.rs']
core=(root/'crates/core/src/engine.rs').read_text()
new='''                        assert_eq!(
                            resolved.udp.session_check_interval,
                            std::time::Duration::from_secs(1)
                        );'''
old='''                        assert_eq!(resolved.udp.session_check_interval, Duration::from_secs(1));'''
assert core.count(new)==1
assert hashlib.sha256(core.replace(new,old,1).encode()).hexdigest()==files['crates/core/src/engine.rs']
prefix=core.split('#[cfg(test)]\nmod tests {')[0]
assert hashlib.sha256(prefix.encode()).hexdigest()=='da57edfb8df6274c5b50972dfb8685a0a8f8933ce7f4462a124667cdcca1c69e'
assert hashlib.sha256((root/'Cargo.lock').read_bytes()).hexdigest()=='a74cb9ffccd98f7365cc8531da73d627e4a57af28c3a8d957bec29a404863cc9'
assert 'f99761138b23f267ebc7621057afe02018ab7c18'in(root/'Cargo.toml').read_text()
records=[]
def run(name,args,structured=False):
 started=time.monotonic()
 with(out/(name+'.log')).open('wb')as log:
  if structured:
   with(out/(name+'.stderr.log')).open('wb')as err:r=subprocess.run(args,cwd=root,env=env,stdout=log,stderr=err,timeout=3000)
  else:r=subprocess.run(args,cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT,timeout=120)
 records.append(dict(stage=name,command=args,exit=r.returncode,seconds=time.monotonic()-started))
 (out/'settings-stage-results.json').write_text(json.dumps(records,indent=2)+'\n')
 assert r.returncode==0,name
run('core-libtest-compile',['cargo','test','--locked','-p','ppvpn-core','--lib','--features','internal-diagnostics','--no-run','--message-format=json'],True)
messages=[json.loads(line)for line in(out/'core-libtest-compile.log').read_text().splitlines()if line.startswith('{')]
paths={r['executable']for r in messages if r.get('reason')=='compiler-artifact'and r.get('target',{}).get('name')=='ppvpn_core'and r.get('profile',{}).get('test')and r.get('executable')}
assert len(paths)==1
binary=pathlib.Path(paths.pop());binary_sha=hashlib.sha256(binary.read_bytes()).hexdigest()
selectors=['engine::tests::relay32_is_only_for_ios_tun_with_an_explicit_256_cap','engine::tests::mobile_runs_sails_mobile_profile_and_the_hosts_cap','engine::tests::cap256_settings_reach_actual_running_sail_snapshot']
for index,selector in enumerate(selectors):
 name='settings-'+str(index+1);run(name,[str(binary),'--exact',selector,'--test-threads=1','--nocapture'])
 assert re.search(r'test result: ok\. 1 passed; 0 failed; 0 ignored;', (out/(name+'.log')).read_text())
assert hashlib.sha256(binary.read_bytes()).hexdigest()==binary_sha
assert all(hashlib.sha256((root/name).read_bytes()).hexdigest()==sha for name,sha in current.items())
receipt=dict(status='PASS',product_source=reference['original_product_source'],validation_source=source,validation_tree=subprocess.check_output(['git','rev-parse','HEAD^{tree}'],cwd=root,text=True).strip(),validation_controller=os.environ['GITHUB_SHA'],run_id=int(os.environ['GITHUB_RUN_ID']),sdk='f99761138b23f267ebc7621057afe02018ab7c18',cargo_lock_sha256=current['Cargo.lock'],production_engine_prefix_sha256=hashlib.sha256(prefix.encode()).hexdigest(),only_cfgtest_Duration_qualification_changed=True,all_other_tracked_files_identical_to_product_source=True,binary_sha256=binary_sha,same_binary_pre_post_verified=True,source_pre_post_verified=True,actual_SDK_parse_and_resolve_cases=105,selectors=[dict(selector=s,passed=1,failed=0,ignored=0)for s in selectors],steps=records,product_App_rebuilt=False,product_UUID_or_SourceManifest_relabelled=False,scope='Actual same macOS platform SDK parse/resolve and empty running256snapshot; original Apple App/source89 unchanged. No physical iOS traffic, memory, throughput or256 safety acceptance')
(out/'actual-sdk-settings-validation.json').write_text(json.dumps(receipt,indent=2)+'\n')
print('Real SDK settings105cases plus existing mobile settings and running256snapshot:3 selectors passed; original App not rebuilt')
