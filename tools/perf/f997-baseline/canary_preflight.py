"""Linux-only, owner-checked no-traffic resource/setsid cleanup preflight."""
import json,os,pathlib,sys,time
from cleanup import cmd

def one_canary(bundle,out,registry,env,mode):
 d=json.loads(registry.read_text());owner=d['owner'];unit=d['canary_units'][mode];ns=d['namespaces'][0];record=out/('canary-'+mode+'-pids.json')
 assert all((pathlib.Path(p)/'owner').read_text().strip()==owner for p in d['claims'])
 for name in d['namespaces']:assert name not in {r.split()[0] for r in cmd(['ip','netns','list'],True).stdout.splitlines()}
 args=['systemd-run','--unit',unit,'--slice',d['slice'],'--property=RuntimeMaxSec='+('5s' if mode=='timeout' else '10s'),'--property=KillMode=control-group','--property=Environment=PPVPN_PERF_OWNER='+owner,'--property=StandardOutput=append:'+str(out/('canary-'+mode+'.original.log')),'--property=StandardError=append:'+str(out/('canary-'+mode+'.original.log'))]
 for k in ['NETEM_NS','NETEM_NET','NETEM_LOCK_DIR','NETEM_LOCK_OWNER']:args+=['--setenv='+k+'='+env[k]]
 args +=[sys.executable,str(pathlib.Path(__file__).with_name('canary_payload.py')),str(bundle/'harness/netns.sh'),ns,str(record)]
 started=cmd(args,True);(out/('canary-'+mode+'-start.stdout')).write_text(started.stdout);(out/('canary-'+mode+'-start.stderr')).write_text(started.stderr)
 deadline=time.monotonic()+5
 while not record.exists():
  assert time.monotonic()<deadline,'canary setup timed out'
  time.sleep(0.05)
 pids=json.loads(record.read_text());assert set(pids)=={'child','grandchild'} and len(set(pids.values()))==2
 cgroup=cmd(['systemctl','show',unit,'-p','ControlGroup','--value'],True).stdout.strip();assert cgroup.endswith('/'+unit)
 for pid in pids.values():
  until=time.monotonic()+1
  while os.getsid(pid)!=pid:
   assert time.monotonic()<until,'canary did not detach session'
   time.sleep(0.02)
  assert cgroup in pathlib.Path('/proc/'+str(pid)+'/cgroup').read_text()
 assert set(map(int,cmd(['ip','netns','pids',ns],True).stdout.split()))==set(pids.values())
 before={name:{'pids':cmd(['ip','netns','pids',name],True).stdout,'links':cmd(['ip','-n',name,'-j','link'],True).stdout,'qdisc':cmd(['ip','netns','exec',name,'tc','-j','qdisc','show'],True).stdout} for name in d['namespaces']}
 assert 'PPVPN_PERF_OWNER='+owner in cmd(['systemctl','show',unit,'-p','Environment','--value'],True).stdout.split()
 if mode=='cancel':cmd(['systemctl','kill','--kill-whom=all','--signal=KILL',unit],True)
 deadline=time.monotonic()+(8 if mode=='timeout' else 2)
 while any(pathlib.Path('/proc/'+str(pid)).exists() for pid in pids.values()):
  assert time.monotonic()<deadline,'canary descendants not proved gone'
  time.sleep(0.05)
 result=cmd(['systemctl','show',unit,'-p','Result','--value'],True).stdout.strip()
 if mode=='timeout':assert result=='timeout','canary did not reach actual RuntimeMaxSec timeout'
 cmd(['systemctl','stop',unit],True)
 group=pathlib.Path('/sys/fs/cgroup')/cgroup.lstrip('/')
 if group.exists():assert dict(x.split() for x in (group/'cgroup.events').read_text().splitlines())['populated']=='0'
 # Verify both the outer lifetime reservation and the harness's own lock.
 assert all((pathlib.Path(p)/'owner').read_text().strip()==owner for p in d['claims'])
 for name in d['namespaces']:assert (pathlib.Path(d['netem_lock_dir'])/('netns-'+name)/'owner').read_text().strip()==owner
 down=['env',*['%s=%s'%(k,env[k]) for k in ['NETEM_NS','NETEM_NET','NETEM_LOCK_DIR','NETEM_LOCK_OWNER']],str(bundle/'harness/netns.sh'),'down'];r=cmd(down,True)
 assert not set(d['namespaces']).intersection(x.split()[0] for x in cmd(['ip','netns','list'],True).stdout.splitlines())
 links={x['ifname'] for x in json.loads(cmd(['ip','-j','link'],True).stdout)};assert not links.intersection(d['host_interfaces'])
 assert all(not (pathlib.Path(d['netem_lock_dir'])/('netns-'+name)).exists() for name in d['namespaces'])
 assert sorted(x['ifname'] for x in json.loads(cmd(['ip','-j','link','show','type','ifb'],True).stdout))==d['initial_host_ifb']
 receipt={'scope':'OWN_CANARY_NO_TRAFFIC_ONLY','mode':mode,'systemd_result':result,'owner':owner,'unit':unit,'actual_detached_child_and_grandchild':pids,'actual_cgroup':cgroup,'namespace_before_deletion':before,'descendants_proved_gone':True,'owned_namespace_interfaces_qdisc_and_inner_locks_gone':True,'outer_lifetime_claims_retained_for_four_cases':True,'host_IFB_unchanged':True,'down_exit':r.returncode}
 (out/('canary-'+mode+'-preflight.actual.json')).write_text(json.dumps(receipt,indent=2)+'\n')
 return receipt

def run_canary(bundle,out,registry,env):
 import hashlib
 receipts=[one_canary(bundle,out,registry,env,mode) for mode in ['cancel','timeout']]
 d=json.loads(registry.read_text())
 r={'scope':'OWN_CANARY_CANCEL_AND_ACTUAL_TIMEOUT_NO_TRAFFIC','run_id':d['run_id'],'attempt':d['attempt'],'owner':d['owner'],'passed':True,'modes':receipts,'code_hashes':{n:hashlib.sha256(pathlib.Path(__file__).with_name(n).read_bytes()).hexdigest() for n in ['canary_payload.py','canary_preflight.py','lease.py','cleanup.py','managed_wrapper.py']}}
 (out/'canary-modes.actual.json').write_text(json.dumps(r,indent=2)+'\n');return r
