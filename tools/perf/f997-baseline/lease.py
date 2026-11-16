"""One owner-bound transient cgroup/namespace lifetime; no production network."""
import json,os,pathlib,secrets,subprocess,sys,time
from cleanup import cmd,cleanup

def atomic(path,data):
 p=path.with_suffix('.new');p.write_text(json.dumps(data,indent=2)+'\n');p.chmod(0o600);os.replace(p,path)

class Lease:
 def __init__(self,out,phase,cpus,job_start,memory=2147483648,tasks=256,cleanup_at=1170,network=True):
  self.out=pathlib.Path(out);self.out.mkdir(parents=True,exist_ok=False);self.out.chmod(0o700);self.registry=self.out/'owner-registry.json';self.closed=False;self.cleanup_result=None
  run=os.environ['GITHUB_RUN_ID'];attempt=os.environ['GITHUB_RUN_ATTEMPT'];assert run.isdecimal() and attempt.isdecimal()
  self.owner='gh'+run+'a'+attempt+'-'+phase+'-'+secrets.token_hex(6);stem='ppvpn-perf-'+self.owner
  ns=100+int(run)%899;net=100+int(run)%140;names=['nc'+str(ns),'ns'+str(ns)] if network else [];devs=[n+'v'+str(i) for n in names for i in (0,1)]
  self.units=[stem+'-c'+str(i)+'.service' for i in range(4)];self.canary_units={m:stem+'-canary-'+m+'.service' for m in ['cancel','timeout']};self.build_unit=stem+'-build.service'
  existing={r.split()[0] for r in cmd(['ip','netns','list'],True).stdout.splitlines()};assert not existing.intersection(names)
  links={x['ifname'] for x in json.loads(cmd(['ip','-j','link'],True).stdout)};assert not links.intersection(devs)
  nets=['10.'+str(net+i)+'.0.0/24' for i in range(3)] if network else []
  import ipaddress
  for row in json.loads(cmd(['ip','-j','route','show','table','all'],True).stdout) if network else []:
   dst=row.get('dst','default')
   if dst!='default':
    n=ipaddress.ip_network(dst,strict=False);assert n.version!=4 or not any(n.overlaps(ipaddress.ip_network(k)) for k in nets)
  lockroot=pathlib.Path('/run/lock/ppvpn-f997-perf');lockroot.mkdir(exist_ok=True)
  claims=[str(lockroot/n) for n in ['ns-'+x for x in names]+['network-'+x.replace('/','-') for x in nets]]
  netlocks=self.out/'netem-locks';netlocks.mkdir()
  self.env={'NETEM_NS':str(ns),'NETEM_NET':str(net),'NETEM_LOCK_DIR':str(netlocks),'NETEM_LOCK_OWNER':self.owner}
  self.d={'owner':self.owner,'run_id':run,'attempt':attempt,'phase':phase,'slice':stem+'.slice','case_units':[stem+'-anchor.service',*self.canary_units.values(),*self.units,self.build_unit],'watch_unit':stem+'-watch','namespaces':names,'host_interfaces':devs,'claims':claims,'initial_namespaces_absent':True,'initial_host_interfaces_absent':True,'netem_lock_dir':str(netlocks),'initial_host_ifb':sorted(x['ifname'] for x in json.loads(cmd(['ip','-j','link','show','type','ifb'],True).stdout)),'cpus':cpus,'memory_max':memory,'tasks_max':tasks,'cleanup_at':cleanup_at,'job_start':job_start,'canary_units':self.canary_units}
  atomic(self.registry,self.d)
 def __enter__(self):
  try:
   d=self.d;left=d['cleanup_at']-(time.monotonic()-d['job_start']);assert left>30
   cmd(['systemd-run','--unit',d['watch_unit'],'--on-active='+str(int(left))+'s','--property=RuntimeMaxSec=25s','--property=Environment=PPVPN_PERF_OWNER='+self.owner,sys.executable,str(pathlib.Path(__file__).with_name('cleanup.py')),'--registry',str(self.registry),'--owner',self.owner],True)
   for item in d['claims']:
    p=pathlib.Path(item);p.mkdir();(p/'owner').write_text(self.owner+'\n')
   cmd(['systemd-run','--unit',d['case_units'][0],'--slice',d['slice'],'--property=RuntimeMaxSec='+str(int(left))+'s','--property=Environment=PPVPN_PERF_OWNER='+self.owner,'/bin/sleep',str(int(left))],True)
   for prop in ['MemoryMax='+str(d['memory_max']),'MemorySwapMax=0','CPUQuota=400%','AllowedCPUs='+','.join(map(str,d['cpus'])),'TasksMax='+str(d['tasks_max'])]:cmd(['systemctl','set-property','--runtime',d['slice'],prop],True)
   cg=cmd(['systemctl','show',d['slice'],'-p','ControlGroup','--value'],True).stdout.strip();assert cg;d['control_group']=cg;atomic(self.registry,d)
   p=pathlib.Path('/sys/fs/cgroup')/cg.lstrip('/');raw={n:(p/n).read_text() for n in ['memory.max','memory.swap.max','memory.events','cpu.max','cpuset.cpus.effective','pids.max']}
   assert raw['memory.max'].strip()==str(d['memory_max']) and raw['memory.swap.max'].strip()=='0'
   quota,period=map(int,raw['cpu.max'].split());assert quota==4*period
   effective=set()
   for w in raw['cpuset.cpus.effective'].strip().split(','):
    lo,_,hi=w.partition('-');effective.update(range(int(lo),int(hi or lo)+1))
   assert effective==set(d['cpus']) and raw['pids.max'].strip()==str(d['tasks_max'])
   atomic(self.out/'resource-limit-readback.json',raw);return self
  except BaseException:
   self.close();raise
 def close(self):
  if self.closed:return self.cleanup_result
  try:
   self.cleanup_result=cleanup(self.registry,self.owner)
   assert not self.cleanup_result['errors'] and not self.cleanup_result.get('oom_occurred',False)
   assert self.cleanup_result['fresh_final_empty_proof']['passed'] is True
   return self.cleanup_result
  except BaseException as ex:
   atomic(self.out/'CLEANUP-INCOMPLETE.json',{'status':'INCOMPLETE_NOT_CLEAN','owner':self.owner,'error':str(ex)});raise
  finally:self.closed=True
 def __exit__(self,typ,value,traceback):self.close();return False
