"""Exercise actual cache seeding and mutation isolation with public fixtures."""
import pathlib,tempfile,unittest
from private_build_isolation import clone_target,child_environment
class IsolationTests(unittest.TestCase):
 def test_mutation_does_not_modify_ordinary_cache(self):
  with tempfile.TemporaryDirectory() as work:
   root=pathlib.Path(work);normal=root/'ordinary';normal.mkdir();(normal/'artifact').write_bytes(b'public fixture');(normal/'alias').symlink_to(normal/'artifact')
   private=root/'private';mode=clone_target(normal,private,root/'seed.log')
   if mode.startswith('APFS'):
    self.assertNotEqual((normal/'artifact').stat().st_ino,(private/'artifact').stat().st_ino)
    self.assertFalse((private/'alias').exists())
   (private/'artifact').write_bytes(b'private fixture mutation')
   self.assertEqual((normal/'artifact').read_bytes(),b'public fixture')
 def test_external_compiler_caches_removed_go_unchanged(self):
  env=dict(RUSTC_WRAPPER='sccache',RUSTC_WORKSPACE_WRAPPER='other',SCCACHE_DIR='/external',SCCACHE_BUCKET='external',CCACHE_DIR='/external',GOCACHE='/configured',GOMODCACHE='/modules',GOTMPDIR='/go-temp',GOBIN='/go-bin')
  actual=child_environment(env)
  self.assertEqual(actual,{**{k:v for k,v in env.items() if k.startswith('GO')},'RUSTC_WRAPPER':'','RUSTC_WORKSPACE_WRAPPER':''})
if __name__=='__main__':unittest.main()
