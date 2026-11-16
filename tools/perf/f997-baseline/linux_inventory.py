"""Strict IFB inventory from the official complete detailed link inventory."""
import json,pathlib
COMMAND=['ip','-j','-d','link','show']
def parse_links(raw):
 def pairs(items):
  d={}
  for k,v in items:
   if k in d:raise ValueError('duplicate inventory key')
   d[k]=v
  return d
 def bad(v):raise ValueError('nonfinite inventory value')
 rows=json.loads(raw,object_pairs_hook=pairs,parse_constant=bad)
 if not isinstance(rows,list) or not rows:raise ValueError('full host link inventory missing or empty')
 names=set()
 for row in rows:
  if not isinstance(row,dict) or not isinstance(row.get('ifname'),str) or not row['ifname']:raise ValueError('full link row missing ifname')
  if row['ifname'] in names:raise ValueError('duplicate interface')
  names.add(row['ifname'])
  if 'linkinfo' in row:
   v=row['linkinfo']
   if not isinstance(v,dict) or not isinstance(v.get('info_kind'),str) or not v['info_kind']:raise ValueError('malformed detailed link kind')
 return rows

def read_links(run,record):
 p=run(COMMAND,False)
 # Persist original stdout/stderr BEFORE interpreting the host's schema.
 path=pathlib.Path(record);path.write_text(p.stdout);path.with_suffix(path.suffix+'.stderr').write_text(p.stderr)
 if p.returncode!=0:raise ValueError('official full link inventory failed: '+str(p.returncode))
 if p.stderr.strip():raise ValueError('official full link inventory has stderr; state unknown')
 return parse_links(p.stdout)

def ifb_names(rows):return sorted(r['ifname'] for r in rows if r.get('linkinfo',{}).get('info_kind')=='ifb')
