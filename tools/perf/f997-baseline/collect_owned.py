"""Copy only this job's explicitly owned evidence roots; originals stay root-owned."""
import argparse,hashlib,json,os,pathlib,shutil,stat
ALLOWED={'f997-smoke-root','f997-build-resource','f997-runtime-output'}
def validate_paths(temp,source,dest):
 temp=pathlib.Path(temp).resolve(strict=True);src=pathlib.Path(source);dst=pathlib.Path(dest)
 if src.is_symlink() or src.resolve(strict=True).parent!=temp or src.name not in ALLOWED:raise ValueError('source is not an explicitly scoped job evidence root')
 if dst.exists() or dst.parent.resolve(strict=True)!=temp or not dst.name.startswith('f997-collected-'):raise ValueError('destination must be a new scoped runner evidence directory')
 return src,dst

def validate_owner(d,run,attempt):
 if d.get('run_id')!=run or d.get('attempt')!=attempt or not isinstance(d.get('owner'),str) or not d['owner'].startswith('gh'+run+'a'+attempt+'-'):raise ValueError('evidence owner is not this job')

def collect(temp,source,dest,run,attempt):
 assert os.geteuid()==0
 uid=int(os.environ['SUDO_UID']);gid=int(os.environ['SUDO_GID']);assert uid>0 and gid>0
 src,dst=validate_paths(temp,source,dest);assert src.stat().st_uid==0
 envelope=next((p for n in ['JOB-OWNER.json','INIT-OWNER.json'] if (p:=src/n).is_file()),None)
 if envelope is None:raise ValueError('no root-owned initialization/job envelope; do not adopt this directory')
 assert not envelope.is_symlink() and envelope.stat().st_uid==0 and envelope.stat().st_size<=8192
 validate_owner(json.loads(envelope.read_text()),run,attempt)
 dst.mkdir(mode=0o700);os.chown(dst,uid,gid);copied={};excluded=[]
 for p in src.rglob('*'):
  rel=p.relative_to(src);q=dst/rel
  if p.is_symlink():raise ValueError('symlink in owned evidence root; no traversal')
  if p.name=='key.pem':excluded.append(str(rel));continue
  if p.is_dir():q.mkdir(mode=0o700,exist_ok=True);os.chown(q,uid,gid)
  elif p.is_file():
   if not stat.S_ISREG(p.stat().st_mode):raise ValueError('nonregular evidence')
   q.parent.mkdir(mode=0o700,exist_ok=True,parents=True);shutil.copyfile(p,q);q.chmod(0o600);os.chown(q,uid,gid);copied[str(rel)]={'bytes':q.stat().st_size,'sha256':hashlib.sha256(q.read_bytes()).hexdigest()}
  else:raise ValueError('unknown evidence file type')
 for p in [dst,*dst.rglob('*')]:
  if p.is_dir():p.chmod(0o700);os.chown(p,uid,gid)
  elif p.is_file():p.chmod(0o600);os.chown(p,uid,gid)
 receipt={'run_id':run,'attempt':attempt,'source_root_basename':src.name,'destination_root_basename':dst.name,'copied_relative_files':copied,'excluded_test_private_keys':excluded,'original_root_ownership_unchanged':src.stat().st_uid==0,'runner_owned_destination_mode':oct(dst.stat().st_mode&0o777)}
 p=dst/'COLLECTION-RECEIPT.json';p.write_text(json.dumps(receipt,indent=2)+'\n');p.chmod(0o600);os.chown(p,uid,gid);return receipt
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--temp-root',required=True);p.add_argument('--source-root',required=True);p.add_argument('--destination-root',required=True);p.add_argument('--run-id',required=True);p.add_argument('--run-attempt',required=True);a=p.parse_args();print(json.dumps(collect(a.temp_root,a.source_root,a.destination_root,a.run_id,a.run_attempt)))
