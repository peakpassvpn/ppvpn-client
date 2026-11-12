"""Exercise owner success/failure/cancel cleanup with a strictly inert fake gh."""
import hashlib,json,os,pathlib,signal,subprocess,sys,tempfile,time,unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]
FAKE='''#!/usr/bin/env python3
import json,os,pathlib,sys,time
p=pathlib.Path(os.environ['FAKE_GH_ROOT']);f=p/'store.json';s=json.loads(f.read_text());a=sys.argv[1:]
with (p/'commands.jsonl').open('a') as log:log.write(json.dumps(a)+'\\n')
def save():f.write_text(json.dumps(s))
if a[:2]==['secret','list']:print(json.dumps([{'name':s['name']}] if s['exists'] else []))
elif a[:2]==['secret','set']:
 b=json.load(sys.stdin);assert set(b)=={'schema','source','controller','dispatch_nonce','build','policy_base64','policy_sha256','domain_capability_base64','domain_capability_sha256','recipient','expires_unix'};assert 'AGE-SECRET-KEY-' not in json.dumps(b)
 assert b['schema']==3 and len(b['dispatch_nonce'])==64
 s['nonce']=b['dispatch_nonce'];s['exists']=True;s['puts']+=1;save()
 if s['mode']=='cancel-set' and s['puts']==1:(p/'ready').touch();time.sleep(60)
elif a[:2]==['secret','delete']:s['exists']=False;s['deletes']+=1;save()
elif a[:2]==['workflow','run']:
 assert a[a.index('private_dispatch_nonce='+s['nonce'])]=='private_dispatch_nonce='+s['nonce']
 print('https://github.com/peakpassvpn/ppvpn-client/actions/runs/123')
elif a[0]=='api':
 path=a[1]
 if '/git/ref/' in path:print(json.dumps({'object':{'sha':'b'*40}}))
 elif path.endswith('/jobs?per_page=100'):print(json.dumps({'jobs':[{'id':1,'name':'sdk2a9_platforms (ios, macos-15, aarch64-apple-ios)','status':'completed','conclusion':'success'}]}))
 elif path.endswith('/runs/123'):
  s['run_reads']+=1;save()
  if s['mode']=='cancel-monitor' and s['run_reads']>=2:(p/'ready').touch();time.sleep(60)
  print(json.dumps({'head_sha':('c' if s['mode']=='wrong-run' else 'b')*40,'event':'workflow_dispatch','status':'completed','conclusion':'success'}))
 else:raise RuntimeError('Unexpected fake endpoint')
else:raise RuntimeError('Unexpected fake command')
'''
class OwnerTests(unittest.TestCase):
 def run_case(self,mode):
  with tempfile.TemporaryDirectory() as work:
   p=pathlib.Path(work);binary=p/'bin';binary.mkdir();gh=binary/'gh';gh.write_text(FAKE);gh.chmod(0o755)
   name=(ROOT/'tools/apple/private_ios256_secret_name.txt').read_text().strip();initial=dict(name=name,exists=mode=='existing',puts=0,deletes=0,run_reads=0,mode=mode);(p/'store.json').write_text(json.dumps(initial))
   policy=p/'policy.txt';policy.write_bytes(b'127.0.0.1:9001\n');sha=hashlib.sha256(policy.read_bytes()).hexdigest();out=p/'private'
   domain=p/'domain.json';domain.write_text(json.dumps(dict(schema=1,kind='CAPACITY_V3_MAIN_DOMAIN_BUILD_INPUT',campaign_contract='CAPACITY_V3_TCP_UDP_CANCEL',package='com.peakpassvpn.ppvpn',protocol='udp',ordinary_enabled=False,public_authority='UNBOUND_BUILD_INPUT_ONLY',runtime_target_manifest='Requires new runtime grant',same_private_policy_sha256=sha,exact_domain='probe.fixture.invalid',port=9001,target_ipv4='127.0.0.1')));domain_sha=hashlib.sha256(domain.read_bytes()).hexdigest()
   env=dict(os.environ,PATH=str(binary)+':'+os.environ['PATH'],FAKE_GH_ROOT=str(p))
   args=[sys.executable,str(ROOT/'tools/apple/run_private_platforms.py'),'--source','a'*40,'--controller','b'*40,'--branch','codex/transport-fixture','--secret-name',name,'--policy',str(policy),'--policy-sha256',sha,'--domain-capability',str(domain),'--domain-capability-sha256',domain_sha,'--private-directory',str(out),'--origin','https://example.invalid']
   if mode.startswith('cancel-'):
    process=subprocess.Popen(args,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    deadline=time.monotonic()+10
    while not (p/'ready').exists() and time.monotonic()<deadline:
     if process.poll() is not None:break
     time.sleep(.02)
    self.assertTrue((p/'ready').exists());process.send_signal(signal.SIGTERM);stdout,stderr=process.communicate(timeout=10);code=process.returncode
   else:
    r=subprocess.run(args,env=env,capture_output=True,timeout=15);code=r.returncode
   store=json.loads((p/'store.json').read_text());state=json.loads((out/'state.json').read_text())
   self.assertEqual(hashlib.sha256(policy.read_bytes()).hexdigest(),sha)
   if mode=='existing':self.assertTrue(store['exists']);self.assertEqual(store['puts'],0);self.assertEqual(store['deletes'],0)
   else:self.assertFalse(store['exists']);self.assertTrue(state['secret_deleted_and_list_verified']);self.assertEqual(store['deletes'],1)
   if mode=='success':self.assertEqual(store['puts'],1);self.assertEqual(state['dispatch_nonce'],store['nonce'])
   self.assertEqual(code==0,mode=='success')
 def test_success(self):self.run_case('success')
 def test_wrong_run_failure(self):self.run_case('wrong-run')
 def test_cancel_after_write_unknown_result(self):self.run_case('cancel-set')
 def test_cancel_during_monitor(self):self.run_case('cancel-monitor')
 def test_never_overwrite_existing(self):self.run_case('existing')
if __name__=='__main__':unittest.main()
