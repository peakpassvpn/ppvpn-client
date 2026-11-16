import hashlib,json,pathlib
root=pathlib.Path(__file__).resolve().parents[3]
for rel,digest in json.loads(pathlib.Path(__file__).with_name('INPUTS.json').read_text()).items():
 p=root/rel;assert p.is_file() and not p.is_symlink() and hashlib.file_digest(p.open('rb'),'sha256').hexdigest()==digest
print('Exact new inputs and unchanged R3 resource sources verified')
