"""Local owner: bind one temporary build secret, dispatch, monitor and delete it.

The age identity stays local. The repository secret is repository-scoped;
only the frozen trusted iOS step references it in this workflow. No PR/fork.
"""
import argparse,base64,hashlib,json,os,pathlib,re,secrets,signal,subprocess,time
from private_domain_capability import validate_domain_capability

def main():
 p=argparse.ArgumentParser();p.add_argument('--source',required=True);p.add_argument('--controller',required=True);p.add_argument('--branch',required=True);p.add_argument('--secret-name',required=True);p.add_argument('--policy',type=pathlib.Path,required=True);p.add_argument('--policy-sha256',required=True);p.add_argument('--domain-capability',type=pathlib.Path,required=True);p.add_argument('--domain-capability-sha256',required=True);p.add_argument('--private-directory',type=pathlib.Path,required=True);p.add_argument('--origin',required=True);p.add_argument('--platforms',choices=['all','ios'],default='ios');p.add_argument('--ordinary-build',type=int,default=355);a=p.parse_args()
 assert all(re.fullmatch('[0-9a-f]{40}',v) for v in [a.source,a.controller])
 assert re.fullmatch('codex/[a-z0-9-]+',a.branch) and re.fullmatch('PPVPN_IOS256_F997_[0-9A-F]{8}',a.secret_name)
 assert re.fullmatch('[0-9a-f]{64}',a.policy_sha256)
 assert a.secret_name==pathlib.Path(__file__).with_name('private_ios256_secret_name.txt').read_text().strip(),'Secret must match frozen workflow reference'
 raw=a.policy.read_bytes();assert hashlib.sha256(raw).hexdigest()==a.policy_sha256 and 0<len(raw)<=128
 domain_raw=a.domain_capability.read_bytes();assert hashlib.sha256(domain_raw).hexdigest()==a.domain_capability_sha256
 validate_domain_capability(domain_raw,raw,a.policy_sha256)
 os.umask(0o077);work=a.private_directory;work.mkdir(mode=0o700,exist_ok=False)
 repo='peakpassvpn/ppvpn-client';state={'schema':1,'source':a.source,'controller':a.controller,'branch':a.branch,'secret_name':a.secret_name,'policy_sha256':a.policy_sha256,'policy_path':str(a.policy),'domain_capability_sha256':a.domain_capability_sha256,'domain_capability_path':str(a.domain_capability),'ordinary_build':a.ordinary_build,'diagnostic_build':a.ordinary_build+1,'status':'prepared','secret_attempted':False,'secret_deleted_and_list_verified':False}
 def save():
  (work/'state.json').write_text(json.dumps(state,indent=2)+'\n')
 def gh(args,body=None):
  try:r=subprocess.run(['gh',*args],input=body,capture_output=True,timeout=60)
  except subprocess.TimeoutExpired:raise RuntimeError('GitHub operation timed out; sensitive diagnostics withheld') from None
  if r.returncode:raise RuntimeError('GitHub operation failed; sensitive diagnostics withheld')
  return r.stdout
 def api(path):return json.loads(gh(['api',f'repos/{repo}/'+path]))
 def names():return {v['name'] for v in json.loads(gh(['secret','list','--repo',repo,'--json','name']))}
 def put(bundle):gh(['secret','set',a.secret_name,'--repo',repo],json.dumps(bundle).encode())
 def cleanup():
  if not state['secret_attempted'] or state['secret_deleted_and_list_verified']:return
  for attempt in range(5):
   try:
    if a.secret_name in names():gh(['secret','delete',a.secret_name,'--repo',repo,'--yes'])
    assert a.secret_name not in names();state['secret_deleted_and_list_verified']=True;save();print('Task build secret deleted and list verified',flush=True);return
   except Exception:
    if attempt==4:state['cleanup_requires_recovery']=True;save();raise RuntimeError('Task secret cleanup needs recovery; see private state') from None
    time.sleep(2)
 def interrupted(signum,frame):raise KeyboardInterrupt('Owner watcher interrupted')
 signal.signal(signal.SIGINT,interrupted);signal.signal(signal.SIGTERM,interrupted)
 save();assert api('git/ref/heads/'+a.branch)['object']['sha']==a.controller
 assert a.secret_name not in names(),'Never overwrite an existing secret'
 identity=work/'age-identity.txt'
 subprocess.run(['age-keygen','-o',str(identity)],check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE);identity.chmod(0o600)
 recipient=subprocess.check_output(['age-keygen','-y',str(identity)],text=True).strip();assert re.fullmatch('age1[a-z0-9]{50,200}',recipient)
 (work/'age-recipient.txt').write_text(recipient+'\n')
 nonce=secrets.token_hex(32);state['dispatch_nonce']=nonce;save()
 bundle={'schema':3,'source':a.source,'controller':a.controller,'dispatch_nonce':nonce,'build':a.ordinary_build+1,'policy_base64':base64.b64encode(raw).decode(),'policy_sha256':a.policy_sha256,'domain_capability_base64':base64.b64encode(domain_raw).decode(),'domain_capability_sha256':a.domain_capability_sha256,'recipient':recipient,'expires_unix':time.time()+6*3600}
 # Record recovery before any write whose network response could be lost.
 state['secret_attempted']=True;save()
 try:
  put(bundle)
  output=gh(['workflow','run','apple-archive-acceptance.yml','--repo',repo,'--ref',a.branch,'-f','sdk2a9_build=true','-f','sdk2a9_platforms='+a.platforms,'-f','source_ref='+a.source,'-f','build_number='+str(a.ordinary_build),'-f','candidate_only=true','-f','candidate_lane=isolated','-f','api_origin='+a.origin,'-f','platform=ios','-f','ios256_variant=diagnostic','-f','private_dispatch_nonce='+nonce]).decode()
  match=re.search(r'/actions/runs/(\d+)',output);assert match,'Dispatch returned no unique run URL'
  run_id=int(match[1]);state['run']=run_id;save();run=api(f'actions/runs/{run_id}');assert run['head_sha']==a.controller and run['event']=='workflow_dispatch'
  state.update(run=run_id,status='running');save()
  print(json.dumps(dict(run=run_id,source=a.source,controller=a.controller,status='running')),flush=True)
  deadline=time.monotonic()+270*60;last=None
  while True:
   if time.monotonic()>deadline:raise TimeoutError('Bounded run monitor expired')
   run=api(f'actions/runs/{run_id}');assert run['head_sha']==a.controller
   jobs=api(f'actions/runs/{run_id}/jobs?per_page=100')['jobs']
   snapshot=[dict(id=j['id'],name=j['name'],status=j['status'],conclusion=j['conclusion']) for j in jobs if j['name'].startswith('sdk2a9')]
   if snapshot!=last:print(json.dumps(snapshot),flush=True);last=snapshot
   (work/'latest-run.json').write_text(json.dumps(dict(run=run,jobs=jobs),indent=2)+'\n')
   ios=[j for j in jobs if j['name'].startswith('sdk2a9_platforms (ios,')]
   if ios and ios[0]['status']=='completed' and not state['secret_deleted_and_list_verified']:cleanup()
   if run['status']=='completed':
    state.update(status='completed',conclusion=run['conclusion']);save();break
   time.sleep(45)
 finally:
  cleanup()
  assert hashlib.sha256(a.policy.read_bytes()).hexdigest()==a.policy_sha256,'Original local input changed'
  assert hashlib.sha256(a.domain_capability.read_bytes()).hexdigest()==a.domain_capability_sha256,'Original domain build input changed'
  # No local policy copy or key is removed until actual artifact review.
  save()
if __name__=='__main__':main()
