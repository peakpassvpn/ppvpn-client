"""Always-step recheck: explicit state path, no inherited RUNNER_TEMP dependency."""
import argparse,json,os,pathlib
from cleanup import cleanup
from collect_owned import validate_paths,validate_owner

def main():
 ap=argparse.ArgumentParser();ap.add_argument('--temp-root',required=True);ap.add_argument('--state-root',required=True);ap.add_argument('--run-id',required=True);ap.add_argument('--run-attempt',required=True);a=ap.parse_args()
 assert os.geteuid()==0
 src=pathlib.Path(a.state_root)
 if not src.exists():print(json.dumps({'status':'NO_RESOURCE_ROOT_CREATED_NOT_A_CLEANUP_PASS','run_id':a.run_id}));return
 # Validate the scoped source without creating the never-used destination.
 validate_paths(a.temp_root,src,pathlib.Path(a.temp_root)/'f997-collected-validation-only')
 assert src.stat().st_uid==0
 envelope=next((p for n in ['JOB-OWNER.json','INIT-OWNER.json'] if (p:=src/n).is_file()),None);assert envelope is not None and not envelope.is_symlink() and envelope.stat().st_uid==0
 validate_owner(json.loads(envelope.read_text()),a.run_id,a.run_attempt)
 failed=False;rows=[]
 for p in src.rglob('owner-registry.json'):
  try:
   assert not p.is_symlink() and p.stat().st_uid==0;d=json.loads(p.read_text());validate_owner(d,a.run_id,a.run_attempt)
   r=cleanup(p,d['owner']);assert not r['errors'] and not r.get('oom_occurred',False) and r['fresh_final_empty_proof']['passed'];rows.append({'registry_relative':str(p.relative_to(src)),'fresh_cleanup_pass':True})
  except BaseException as ex:
   failed=True;row={'status':'INCOMPLETE_NOT_CLEAN','registry_relative':str(p.relative_to(src)),'error':str(ex)};rows.append(row);p.with_suffix('.always-INCOMPLETE.json').write_text(json.dumps(row)+'\n')
 result={'run_id':a.run_id,'attempt':a.run_attempt,'status':'INCOMPLETE_NOT_CLEAN' if failed else ('FRESH_OWNED_LEASE_EMPTY' if rows else 'NO_REGISTERED_LEASE_NOT_A_CLEANUP_PASS'),'registries':rows,'original_root_ownership_preserved':src.stat().st_uid==0};(src/'ALWAYS-CLEANUP-RECHECK.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result));raise SystemExit(1 if failed else 0)
if __name__=='__main__':main()
