"""Only this exact successful prior build; no compiler or generic cache fallback."""
import hashlib,json,os,pathlib,subprocess,sys,zipfile
HERE=pathlib.Path(__file__).resolve().parent;pin=json.loads((HERE/'REUSE.json').read_text());source=json.loads((HERE/'SOURCE.json').read_text());temp=pathlib.Path(os.environ['RUNNER_TEMP']).resolve(strict=True)
assert sys.platform=='linux' and os.uname().machine=='x86_64' and os.geteuid()!=0
assert os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted' and os.environ['GITHUB_RUN_ID'].isdecimal()
assert pathlib.Path('/sys/fs/cgroup/cgroup.controllers').is_file() and len(os.sched_getaffinity(0))>=4
def api(path):return subprocess.check_output(['gh','api','repos/'+os.environ['GITHUB_REPOSITORY']+'/'+path])
def sha(p):return hashlib.file_digest(p.open('rb'),'sha256').hexdigest()
current=json.loads(api('actions/runs/'+os.environ['GITHUB_RUN_ID']));assert current['head_sha']==os.environ['GITHUB_SHA'] and current['event']=='workflow_dispatch'
meta=json.loads(api('actions/artifacts/'+str(pin['artifact_id'])));assert str(meta['workflow_run']['id'])==str(pin['run']) and meta['workflow_run']['head_sha']==pin['head'] and not meta['expired'] and meta['digest']=='sha256:'+pin['artifact_sha256']
assert meta['size_in_bytes']==pin['artifact_bytes']
jobs=json.loads(api('actions/runs/'+str(pin['run'])+'/jobs'));job=[j for j in jobs['jobs']if j['id']==pin['build_job']];assert len(job)==1 and job[0]['name']==pin['build_job_name'] and job[0]['conclusion']=='success'
raw=api('actions/artifacts/'+str(pin['artifact_id'])+'/zip');assert len(raw)==pin['artifact_bytes'] and hashlib.sha256(raw).hexdigest()==pin['artifact_sha256']
original=temp/'tcp-authenticated-original-build.zip';original.write_bytes(raw);dest=temp/'tcp-authenticated-original-build';dest.mkdir()
with zipfile.ZipFile(original)as z:
 assert z.testzip() is None and len(z.namelist())==len(set(z.namelist()))
 assert all(not pathlib.Path(n).is_absolute() and '..'not in pathlib.Path(n).parts and (z.getinfo(n).external_attr>>16)&0o170000!=0o120000 for n in z.namelist());z.extractall(dest)
bundle=dest/'tcp-build-output';manifest=bundle/'BINARY-MANIFEST.json';assert sha(manifest)==pin['manifest_sha256'];m=json.loads(manifest.read_text());assert {k:m[k]for k in pin['manifest_identity']}==pin['manifest_identity']
assert m['base']==source['base'] and m['tree']==source['tree'] and m['lock_sha256']==source['Cargo_lock_sha256'] and m['source_pre_post_clean']is True
assert m['cargo_profile_config']==source['cargo_profile_config'] and m['profile']==source['profile'] and m['cargo_jobs']==1 and m['build_run']==str(pin['run'])
assert all(type(row['exit'])is int and row['exit']==0 for row in m['stages'])
for name in ['old-compile','new-compile','netgen']:assert len([r for r in m['stages']if r['name']==name and r['exit']==0])==1
for arm,row in m['arms'].items():
 assert row['source_sha256']==source['patched_source'][arm] and row['patch_sha256']==source['patches']['old-bench.patch'if arm=='old'else'candidate.patch'] and row['example_sha256']==source['example_sha256']
 assert sha(bundle/(arm+'.tun.rs'))==row['source_sha256'] and sha(bundle/(arm+'.example.rs'))==source['example_sha256']
 assert (bundle/(arm+'-derived-tree.stdout.raw')).read_text().strip()==row['derived_source_tree']
 toolchain=(bundle/'toolchain.stdout.raw').read_text();assert 'rustc 1.98.1 'in toolchain and 'host: x86_64-unknown-linux-gnu'in toolchain and 'go1.25.8 linux/amd64'in toolchain
 compiler=(bundle/(arm+'-compile.stderr.raw')).read_text();actual=[l for l in compiler.splitlines()if 'rustc --crate-name packet_bridge_tcp_bench 'in l];assert len(actual)==1
 for flag in ['-C opt-level=3','-C lto=off','-C codegen-units=16']:assert flag in actual[0]
 assert '-C debuginfo='not in actual[0]
for rel,row in m['files'].items():
 p=bundle/rel;assert p.resolve().is_relative_to(bundle) and not p.is_symlink() and p.stat().st_size==row['bytes'] and sha(p)==row['sha256']
 if rel.startswith('bin/'):
  header=p.open('rb').read(20);assert header[:6]==b'\x7fELF\x02\x01' and int.from_bytes(header[18:20],'little')==62,'Require actual native Linux x86_64 ELF'
  p.chmod(0o755)
 elif rel=='harness/netns.sh':p.chmod(0o755)
root=dest/'f997-collected-build';assert json.loads((root/'actual-service.exit.json').read_text())['exit']==0
cleanup=json.loads((root/'owner-registry.cleanup.json').read_text());assert not cleanup['errors'] and not cleanup['oom_occurred'] and cleanup['fresh_final_empty_proof']['passed']is True
(temp/'tcp-front-authentication.json').write_text(json.dumps({'origin_run':pin['run'],'origin_head':pin['head'],'origin_artifact':pin['artifact_id'],'ZIP_sha256':pin['artifact_sha256'],'manifest_sha256':pin['manifest_sha256'],'actual_original_build_job':pin['build_job'],'original_build_exit0':True,'source_profile_tree_binary_peer_harness_architecture_hashes_verified':True,'binary_origin':'authenticated_reuse_not_recompiled','new_compile_invocations':0,'current_run':os.environ['GITHUB_RUN_ID'],'actual_parent_architecture':os.uname().machine},indent=2)+'\n')
with open(os.environ['GITHUB_ENV'],'a')as f:
 f.write('PB_TCP_BUNDLE='+str(bundle)+'\n');f.write('FRONT_MANIFEST_SHA256='+pin['manifest_sha256']+'\n')
