import copy,json,unittest
from netgen_capture import strict_result
class StrictNetgen(unittest.TestCase):
 def base(self,n=1):return {'ok':n,'failed':0,'corrupt':0,'streams':n,'bytes':67108864,'seconds':2.0,'mbps':268.435456}
 def args(self,n=1):return ['bulk','-streams',str(n),'-bytes',str(67108864//n),'-dir','down','-target','10.101.0.1:9000']
 def verify(self,d,n=1,code=0,args=None):return strict_result(json.dumps(d).encode(),code,n,args or self.args(n))
 def test_real_schema_one_and_four(self):
  for n in [1,4]:self.assertEqual(self.verify(self.base(n),n)['bytes'],67108864)
  self.assertEqual(self.verify({**self.base(),'errors':[]})['ok'],1)
 def test_process_nonzero_never_green(self):
  for c in [1,124,-9]:
   with self.assertRaises(ValueError):self.verify(self.base(),code=c)
 def test_empty_malformed_multiple_duplicate_nonfinite(self):
  for raw in [b'',b'{}',b'{',b'null',b'[]',b'{}\n{}',b'{"ok":1,"ok":1}',b'{"ok":NaN}',b'{"ok":Infinity}',json.dumps(self.base()).encode()+b' junk']:
   with self.assertRaises((ValueError,TypeError)):strict_result(raw,0,1,self.args())
 def test_harness_exit0_partial_positive_mbps_rejected(self):
  for edit in [{'ok':0,'failed':1,'bytes':0},{'ok':0,'corrupt':1},{'bytes':33554432},{'ok':True},{'ok':1.0},{'errors':['truncated']},{'seconds':0},{'seconds':float('inf')},{'mbps':-1},{'mbps':123},{'streams':4},*[{'errors':v} for v in [False,0,'',{},None]]]:
   d=self.base();d.update(edit)
   with self.assertRaises(ValueError):self.verify(d)
 def test_actual_direction_stream_input_binding(self):
  for args in [self.args()+['-dir','down'],['bulk','-streams','1','-bytes','67108864','-dir','up'],['bulk','-streams','4','-bytes','16777216','-dir','down']]:
   with self.assertRaises((ValueError,IndexError)):self.verify(self.base(),args=args)
class CaptureClosure(unittest.TestCase):
 def test_actual_child_exit_and_original_streams(self):
  import hashlib,os,pathlib,subprocess,sys,tempfile
  good={'ok':1,'failed':0,'corrupt':0,'streams':1,'bytes':67108864,'seconds':2.0,'mbps':268.435456}
  for payload,child_exit,want in [(good,0,True),(good,7,False),({},0,False),({**good,'ok':0,'failed':1},0,False)]:
   with self.subTest(child_exit=child_exit,payload=payload),tempfile.TemporaryDirectory(prefix='ppvpn-netgen-validation-stub-') as td:
    d=pathlib.Path(td);fake=d/'netgen-stub';raw=json.dumps(payload).encode()+b'\n'
    fake.write_text('#!'+sys.executable+'\nimport os,sys\nos.write(1,'+repr(raw)+')\nos.write(2,b"stub-original-stderr\\n")\nsys.exit('+str(child_exit)+')\n');fake.chmod(0o755)
    capture=d/'capture';env=dict(os.environ,PPVPN_REAL_NETGEN=str(fake),PPVPN_NETGEN_SHA256=hashlib.sha256(fake.read_bytes()).hexdigest(),PPVPN_PERF_CAPTURE_DIR=str(capture),PPVPN_EXPECTED_STREAMS='1')
    p=subprocess.run([sys.executable,str(pathlib.Path(__file__).with_name('netgen_capture.py')),'bulk','-streams','1','-bytes','67108864','-dir','down'],capture_output=True,env=env,timeout=10)
    receipts=list(capture.glob('*.receipt.json'));self.assertEqual(len(receipts),1);r=json.loads(receipts[0].read_text());self.assertEqual(r['actual_netgen_exit'],child_exit);self.assertEqual(r['validated'],want);self.assertEqual(p.stdout,raw);self.assertEqual((capture/r['stderr_file']).read_bytes(),b'stub-original-stderr\n');self.assertEqual(p.returncode==0,want)
if __name__=='__main__':unittest.main(verbosity=2)

