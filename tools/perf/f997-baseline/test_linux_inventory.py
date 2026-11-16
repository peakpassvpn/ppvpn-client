import json,pathlib,subprocess,tempfile,unittest
from linux_inventory import COMMAND,parse_links,read_links,ifb_names
class FullLinkInventory(unittest.TestCase):
 def test_full_detailed_inventory_identifies_only_actual_ifb_kind(self):
  rows=[{'ifname':'lo'},{'ifname':'eth0'},{'ifname':'ifb0','linkinfo':{'info_kind':'ifb'}},{'ifname':'veth0','linkinfo':{'info_kind':'veth'}}]
  self.assertEqual(ifb_names(parse_links(json.dumps(rows))),['ifb0'])
 def test_missing_ifname_and_unknown_full_schema_fail_closed(self):
  for rows in [[],[{}],[{'ifindex':1}],{},[{'ifname':''}],[{'ifname':'x'},{'ifname':'x'}],[{'ifname':'x','linkinfo':{}}],[{'ifname':'x','linkinfo':None}]]:
   with self.subTest(rows=rows),self.assertRaises(ValueError):parse_links(json.dumps(rows))
 def test_original_stdout_stderr_saved_before_parse_or_command_failure(self):
  for raw,code in [('[{"ifindex":1}]\n',0),('[{"ifname":"lo"}]\n',1)]:
   with self.subTest(exit=code),tempfile.TemporaryDirectory(prefix='ppvpn-linux-inventory-contract-') as td:
    p=pathlib.Path(td)/'original.json'
    def backend(args,check):
     self.assertEqual(args,['ip','-j','-d','link','show']);self.assertFalse(check);return subprocess.CompletedProcess(args,code,raw,'original-stderr')
    with self.assertRaises(ValueError):read_links(backend,p)
    self.assertEqual(p.read_text(),raw);self.assertEqual(p.with_suffix('.json.stderr').read_text(),'original-stderr')
if __name__=='__main__':unittest.main(verbosity=2)
