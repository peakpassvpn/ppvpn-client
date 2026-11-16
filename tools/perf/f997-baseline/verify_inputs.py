"""Verify relative/hash-only source inputs; no runner environment assumptions."""
import hashlib,json,pathlib
root=pathlib.Path(__file__).resolve().parent;m=json.loads((root/'RELATIVE-INPUTS.json').read_text())
for rel,row in m['relative_files'].items():
 p=root/rel;assert p.resolve().is_relative_to(root) and p.is_file() and not p.is_symlink();assert p.stat().st_size==row['bytes'] and hashlib.sha256(p.read_bytes()).hexdigest()==row['sha256']
print(json.dumps({'reviewed_relative_sources_verified':len(m['relative_files']),'scope':m['scope']}))
