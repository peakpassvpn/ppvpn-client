"""Stub backend contract only; never executes Linux commands."""
import json,pathlib,subprocess,tempfile,unittest
from unittest.mock import patch
import cleanup
class CachedCleanupFreshQuery(unittest.TestCase):
 def execute(self,residue):
  with tempfile.TemporaryDirectory(prefix='ppvpn-cleanup-query-stub-') as td:
   root=pathlib.Path(td);registry=root/'owner-registry.json';d={'owner':'stub-owner','run_id':'123','claims':[str(root/'lease')],'namespaces':['nc777'],'host_interfaces':['nc777v0'],'initial_host_ifb':[],'control_group':'/ppvpn-test-absent-slice','case_units':['stub.service'],'netem_lock_dir':str(root/'inner')};registry.write_text(json.dumps(d));cache=registry.with_suffix('.cleanup.json');original={'owner':'stub-owner','errors':[],'all_owned_namespaces_interfaces_pids_locks_gone':True};cache.write_text(json.dumps(original));before=cache.read_bytes();calls=[]
   def backend(args,**kwargs):
    calls.append(args)
    if args[:3]==['ip','netns','list']:s='nc777 (id: 1)\n' if residue else ''
    elif args==['ip','-j','-d','link','show']:s='[{"ifname":"lo"}]'
    elif args[0]=='systemctl':s=''
    else:raise AssertionError(args)
    return subprocess.CompletedProcess(args,0,s,'')
   with patch.object(cleanup.subprocess,'run',side_effect=backend):r=cleanup.cleanup(registry,'stub-owner')
   self.assertEqual(cache.read_bytes(),before);self.assertIn(['ip','netns','list'],calls);self.assertIn(['ip','-j','-d','link','show'],calls);self.assertIn(['systemctl','list-units','--all','--no-legend','--plain','--no-pager'],calls);self.assertEqual(r['fresh_final_empty_proof']['run_id'],'123');return r
 def test_cached_success_requires_fresh_positive_queries(self):self.assertTrue(self.execute(False)['fresh_final_empty_proof']['passed'])
 def test_new_namespace_residue_invalidates_cached_green(self):
  r=self.execute(True);self.assertFalse(r['all_owned_namespaces_interfaces_pids_locks_gone']);self.assertIn('fresh-empty-proof-failed',r['errors'])
if __name__=='__main__':unittest.main(verbosity=2)
