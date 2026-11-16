"""Independent owner-checked cleanup with bounded fresh official empty proof."""
import argparse,fcntl,hashlib,json,os,pathlib,subprocess,time

def cmd(args,check=False):
 p=subprocess.run(args,capture_output=True,text=True,timeout=15)
 if check and p.returncode:raise RuntimeError((args,p.returncode,p.stderr))
 return p

def fresh_empty(d,run):
 namespaces=run(['ip','netns','list'],True).stdout
 links=json.loads(run(['ip','-j','link'],True).stdout)
 ifb=json.loads(run(['ip','-j','link','show','type','ifb'],True).stdout)
 claims={p:pathlib.Path(p).exists() for p in d['claims']}
 ns_clear=not {x.split()[0] for x in namespaces.splitlines()}.intersection(d['namespaces'])
 links_clear=not {x['ifname'] for x in links}.intersection(d['host_interfaces'])
 cg=d.get('control_group');cg_absent=bool(cg) and not (pathlib.Path('/sys/fs/cgroup')/cg.lstrip('/')).exists()
 inventory=run(['systemctl','list-units','--all','--no-legend','--plain','--no-pager'],True).stdout
 rows={parts[0]:parts[2] for line in inventory.splitlines() if len(parts:=line.split())>=4}
 states={u:rows.get(u,'unloaded') for u in d['case_units']}
 units_clear=all(s in ['inactive','failed','unloaded'] for s in states.values())
 inner={n:(pathlib.Path(d['netem_lock_dir'])/('netns-'+n)).exists() for n in d['namespaces']}
 unchanged=sorted(x['ifname'] for x in ifb)==d['initial_host_ifb']
 return {'queried_unix_ns':time.time_ns(),'owner':d['owner'],'run_id':d['run_id'],'namespace_inventory_original':namespaces,'host_link_inventory_original':links,'host_ifb_inventory_original':ifb,'claim_exists_current':claims,'inner_lock_exists_current':inner,'unit_inventory_original':inventory,'unit_states_current':states,'cgroup_absent_current':cg_absent,'passed':ns_clear and links_clear and units_clear and cg_absent and unchanged and not any(claims.values()) and not any(inner.values())}

def cleanup(registry,owner):
 path=pathlib.Path(registry);d=json.loads(path.read_text());assert d['owner']==owner
 lock=path.with_suffix('.cleanup.lock').open('a');deadline=time.monotonic()+23
 try:
  while True:
   try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB);break
   except BlockingIOError:
    if time.monotonic()>deadline:raise TimeoutError('cleanup locked; INCOMPLETE')
    time.sleep(0.05)
  def run(args,check=False):
   remaining=deadline-time.monotonic()
   if remaining<=0:raise TimeoutError('cleanup deadline; INCOMPLETE')
   p=subprocess.run(args,capture_output=True,text=True,timeout=min(3,remaining))
   if check and p.returncode:raise RuntimeError((args,p.returncode,p.stderr))
   return p
  cached=path.with_suffix('.cleanup.json')
  if cached.exists():
   r=json.loads(cached.read_text());assert r['owner']==owner
   fresh=fresh_empty(d,run);r['fresh_final_empty_proof']=fresh;r['reused_cleanup_operations_only']=True
   if not fresh['passed']:r['errors']=[*r['errors'],'fresh-empty-proof-failed']
   r['all_owned_namespaces_interfaces_pids_locks_gone']=not r['errors'] and fresh['passed']
   path.with_suffix('.reentry-'+str(time.time_ns())+'.json').write_text(json.dumps(r,indent=2)+'\n');return r
  claims=d['claims'];errors=[];actions=[];oom=False
  def owned():return all((pathlib.Path(p)/'owner').exists() and (pathlib.Path(p)/'owner').read_text().strip()==owner for p in claims)
  assert d['initial_namespaces_absent'] and d['initial_host_interfaces_absent']
  for unit in d['case_units']:
   env=run(['systemctl','show',unit,'-p','Environment','--value']).stdout
   if not env.strip():continue
   if 'PPVPN_PERF_OWNER='+owner not in env.split():errors.append('unit-owner-mismatch:'+unit);continue
   run(['systemctl','kill','--kill-whom=all','--signal=KILL',unit]);run(['systemctl','stop',unit]);actions.append('stopped:'+unit)
  time.sleep(0.1);snapshots={}
  for ns in d['namespaces']:
   pids=run(['ip','netns','pids',ns])
   if pids.returncode:continue
   ids=pids.stdout.split();snapshots[ns]={'pids':ids,'links':run(['ip','-n',ns,'-j','link'],True).stdout,'qdisc':run(['ip','netns','exec',ns,'tc','-j','qdisc','show'],True).stdout}
   q=pathlib.Path(d['netem_lock_dir'])/('netns-'+ns)/'owner'
   if q.exists() and q.read_text().strip()!=owner:errors.append('foreign-namespace-lock');continue
   if ids:errors.append('namespace-pids-remain');continue
   if not owned():errors.append('resource-lease-owner-mismatch');continue
   run(['ip','netns','delete',ns],True);actions.append('deleted:'+ns)
  for dev in d['host_interfaces']:
   if run(['ip','link','show','dev',dev]).returncode==0:
    if not owned():errors.append('interface-owner-mismatch');continue
    run(['ip','link','delete','dev',dev],True)
  for ns in d['namespaces']:
   q=pathlib.Path(d['netem_lock_dir'])/('netns-'+ns)
   if q.exists():
    if (q/'owner').exists() and (q/'owner').read_text().strip()==owner:(q/'owner').unlink();q.rmdir()
    else:errors.append('foreign-inner-lock')
  cg=d.get('control_group');counters={}
  if cg:
   base=pathlib.Path('/sys/fs/cgroup')/cg.lstrip('/')
   for n in ['memory.events','memory.peak','memory.max','cpu.stat','cpu.max','cpuset.cpus.effective','cgroup.events']:
    if (base/n).exists():counters[n]=(base/n).read_text()
  if 'memory.events' not in counters or 'cgroup.events' not in counters:errors.append('missing-cgroup-evidence')
  if counters.get('cgroup.events') and dict(x.split() for x in counters['cgroup.events'].splitlines()).get('populated')!='0':errors.append('cgroup-populated')
  events=dict(x.split() for x in counters.get('memory.events','').splitlines());oom=any(int(events.get(k,'0')) for k in ['oom','oom_kill','oom_group_kill'])
  run(['systemctl','stop',d['watch_unit']+'.timer'])
  if cg:run(['systemctl','stop',d['slice']],True)
  for p in reversed(claims):
   if errors:break
   q=pathlib.Path(p)
   if not q.exists():continue
   if not (q/'owner').exists() or (q/'owner').read_text().strip()!=owner:errors.append('foreign-or-unwritten-lease');continue
   (q/'owner').unlink();q.rmdir()
  fresh=fresh_empty(d,run)
  if not fresh['passed']:errors.append('fresh-empty-proof-failed')
  r={'owner':owner,'run_id':d['run_id'],'actions':actions,'before_namespace_deletion':snapshots,'cgroup_counters':counters,'oom_occurred':oom,'errors':errors,'fresh_final_empty_proof':fresh,'all_owned_namespaces_interfaces_pids_locks_gone':not errors and fresh['passed']}
  cached.write_text(json.dumps(r,indent=2)+'\n');return r
 finally:lock.close()
if __name__=='__main__':
 ap=argparse.ArgumentParser();ap.add_argument('--registry',required=True);ap.add_argument('--owner',required=True);a=ap.parse_args()
 try:r=cleanup(a.registry,a.owner);print(json.dumps(r));raise SystemExit(1 if r['errors'] else 0)
 except Exception as ex:
  pathlib.Path(a.registry).with_suffix('.cleanup-INCOMPLETE.json').write_text(json.dumps({'status':'INCOMPLETE_NOT_CLEAN','owner':a.owner,'error':str(ex)})+'\n');raise
