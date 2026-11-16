import pathlib,tempfile,unittest
from collect_owned import validate_paths,validate_owner
class OwnedCollectionGuards(unittest.TestCase):
 def test_explicit_root_and_new_scoped_destination_only(self):
  with tempfile.TemporaryDirectory(prefix='ppvpn-collection-contract-') as td:
   t=pathlib.Path(td);s=t/'f997-smoke-root';s.mkdir();d=t/'f997-collected-smoke';self.assertEqual(validate_paths(t,s,d),(s,d))
   for src,dst in [(t,t/'f997-collected-bad'),(s,t/'unscoped'),(s,s)]:
    with self.assertRaises(ValueError):validate_paths(t,src,dst)
   alias=t/'f997-build-resource';alias.symlink_to(s,target_is_directory=True)
   with self.assertRaises(ValueError):validate_paths(t,alias,d)
 def test_current_job_owner_required(self):
  validate_owner({'run_id':'123','attempt':'1','owner':'gh123a1-smoke-token'},'123','1')
  for d in [{'run_id':'124','attempt':'1','owner':'gh124a1-token'},{'run_id':'123','attempt':'2','owner':'gh123a2-token'},{'run_id':'123','attempt':'1','owner':'other'}]:
   with self.assertRaises(ValueError):validate_owner(d,'123','1')
if __name__=='__main__':unittest.main(verbosity=2)
