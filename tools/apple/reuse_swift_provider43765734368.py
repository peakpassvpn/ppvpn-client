"""Verify exact Swift fixture input equivalence to an actual completed prior stage."""
import pathlib,json,hashlib,sys
p=pathlib.Path(__file__);d=json.loads(p.with_name('swift_provider_reuse43765734368.json').read_text());root=pathlib.Path(sys.argv[1])
for name,digest in d['exact_inputs'].items():assert hashlib.sha256((root/name).read_bytes()).hexdigest()==digest,name
assert d['actual_swift_exit']==0
print(json.dumps(d))
