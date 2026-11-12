"""Real age authentication tests; no repository secret or real policy input."""
import hashlib,importlib.util,io,json,pathlib,subprocess,tarfile,tempfile,unittest
from decrypt_private_ios256 import decrypt
class AgeTransportTests(unittest.TestCase):
 def test_complete_authentication_and_binding_before_extract(self):
  with tempfile.TemporaryDirectory() as work:
   root=pathlib.Path(work);identity=root/'identity';subprocess.run(['age-keygen','-o',str(identity)],check=True,capture_output=True)
   recipient=subprocess.check_output(['age-keygen','-y',str(identity)],text=True).strip()
   sha=hashlib.sha256(b'127.0.0.1:9001\n').hexdigest()
   manifest=dict(source='a'*40,controller='b'*40,run_id=123,dispatch_nonce='c'*64,build=356,domain_capability_input_sha256='d'*64,policy_input_sha256=sha,policy_after_sha256=sha,sdk='f99761138b23f267ebc7621057afe02018ab7c18',base_diagnostics=True,explicit_ios256_candidate=True,udp_target_diagnostics=True,main_app_engine=False,failure_class=None)
   payload=root/'payload.tar.gz'
   with tarfile.open(payload,'w:gz') as tf:
    for name,data in [('private-build-manifest.json',json.dumps(manifest).encode()),('fixture.txt',b'fixture-only')]:
     info=tarfile.TarInfo(name);info.size=len(data);tf.addfile(info,io.BytesIO(data))
   cipher=root/'payload.age';subprocess.run(['age','-r',recipient,'-o',str(cipher),str(payload)],check=True,capture_output=True)
   expected=dict(source='a'*40,controller='b'*40,run_id=123,dispatch_nonce='c'*64,build=356,policy_sha256=sha,domain_capability_sha256='d'*64,encrypted_sha256=hashlib.sha256(cipher.read_bytes()).hexdigest())
   decrypt(cipher,identity,root/'valid',expected);self.assertEqual((root/'valid/verified/fixture.txt').read_bytes(),b'fixture-only')
   raw=cipher.read_bytes()
   for name,data in [('bitflip',raw[:-1]+bytes([raw[-1]^1])),('truncated',raw[:-20])]:
    bad=root/(name+'.age');bad.write_bytes(data);pin=dict(expected,encrypted_sha256=hashlib.sha256(data).hexdigest())
    with self.assertRaises(ValueError):decrypt(bad,identity,root/name,pin)
    self.assertFalse((root/name/'verified').exists());self.assertFalse((root/name/'authenticated.tar.gz.partial').exists())
   with self.assertRaises(AssertionError):decrypt(cipher,identity,root/'wrong-context',dict(expected,run_id=124))
   self.assertFalse((root/'wrong-context/verified').exists())
   with self.assertRaises(AssertionError):decrypt(cipher,identity,root/'wrong-nonce',dict(expected,dispatch_nonce='d'*64))
   self.assertFalse((root/'wrong-nonce/verified').exists())
   with self.assertRaises(AssertionError):decrypt(cipher,identity,root/'wrong-domain',dict(expected,domain_capability_sha256='e'*64))
   self.assertFalse((root/'wrong-domain/verified').exists())
   with self.assertRaises(AssertionError):decrypt(cipher,identity,root/'wrong-policy',dict(expected,policy_sha256='0'*64))
   self.assertFalse((root/'wrong-policy/verified').exists())
if __name__=='__main__':unittest.main()
