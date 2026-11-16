"""R3 platform smoke ONLY: no SDK compile, no throughput workloads."""
import argparse,hashlib,json,os,pathlib,pwd,secrets,signal,subprocess,sys,time
from lease import Lease,atomic
from canary_preflight import run_canary

def main():
 p=argparse.ArgumentParser();p.add_argument('--out',required=True);p.add_argument('--job-start',type=float,required=True);p.add_argument('--runner-path',required=True);a=p.parse_args()
 assert sys.platform=='linux' and os.geteuid()==0 and os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted'
 run=os.environ['GITHUB_RUN_ID'];attempt=os.environ['GITHUB_RUN_ATTEMPT'];assert run.isdecimal() and attempt.isdecimal()
 out=pathlib.Path(a.out).resolve();out.mkdir(exist_ok=False);out.chmod(0o700);atomic(out/'JOB-OWNER.json',{'run_id':run,'attempt':attempt,'owner':'gh'+run+'a'+attempt+'-smoke-'+secrets.token_hex(6),'purpose':'NO_SDK_COMPILE_NO_GENERATED_BENCHMARK_TRAFFIC'})
 cpus=sorted(os.sched_getaffinity(0));assert len(cpus)>=4;cpus=cpus[:4]
 uid=int(os.environ['SUDO_UID']);assert uid>0;user=pwd.getpwuid(uid);passed=False;leases=[]
 def interrupted(n,f):raise SystemExit(128+n)
 signal.signal(signal.SIGTERM,interrupted);signal.signal(signal.SIGINT,interrupted)
 try:
  with Lease(out/'front-resource-probe','frontprobe',cpus,a.job_start,memory=8589934592,tasks=512,cleanup_at=210,network=False) as front:
   leases.append(front)
   env={'HOME':user.pw_dir,'PATH':a.runner_path}
   for k in ['GOCACHE','GOMODCACHE','GOTMPDIR','GOBIN','GOTOOLCHAIN','CARGO_HOME','RUSTUP_HOME']:
    if k in os.environ:env[k]=os.environ[k]
   command=['systemd-run','--wait','--pipe','--unit',front.build_unit,'--slice',front.d['slice'],'--uid',str(uid),'--property=RuntimeMaxSec=30s','--property=MemoryMax=8589934592','--property=MemorySwapMax=0','--property=CPUQuota=400%','--property=AllowedCPUs='+','.join(map(str,cpus)),'--property=TasksMax=512','--property=KillMode=control-group','--property=TimeoutStopSec=2s','--property=Environment=PPVPN_PERF_OWNER='+front.owner]
   command += ['--setenv='+k+'='+v for k,v in env.items()]+[sys.executable,str(pathlib.Path(__file__).with_name('metadata_probe.py'))]
   atomic(out/'metadata-probe.actual-command.json',{'command':command})
   r=subprocess.run(command,capture_output=True,timeout=35);(out/'metadata-probe.original.stdout').write_bytes(r.stdout);(out/'metadata-probe.original.stderr').write_bytes(r.stderr);assert r.returncode==0
   d=json.loads(r.stdout);files=d['cgroup_files'];assert d['uid']==uid and d['PATH']==a.runner_path and d['affinity']==cpus
   assert files['memory.max'].strip()=='8589934592' and files['memory.swap.max'].strip()=='0' and files['pids.max'].strip()=='512'
   quota,period=map(int,files['cpu.max'].split());assert quota==4*period
   assert d['injected_Go_environment']=={k:env.get(k) for k in ['GOCACHE','GOMODCACHE','GOTMPDIR','GOBIN']}
  with Lease(out/'runtime-resource-canary','smokecanary',cpus,a.job_start,memory=2147483648,tasks=256,cleanup_at=210,network=True) as runtime:
   leases.append(runtime);proof=run_canary(pathlib.Path(__file__).parent,runtime.out,runtime.registry,runtime.env);assert proof['passed'] and proof['run_id']==run and proof['owner']==runtime.owner
  assert all(l.cleanup_result['fresh_final_empty_proof']['passed'] and not l.cleanup_result['errors'] for l in leases)
  passed=True
 finally:
  atomic(out/'SMOKE-RESULT.json',{'passed':passed,'run_id':run,'attempt':attempt,'actual_SDK_compilations':0,'actual_throughput_cases':0,'actual_generated_benchmark_load':False,'independent_watchers_armed_count':sum(bool(json.loads(p.read_text()).get('watcher_armed',False)) for p in out.rglob('owner-registry.json')),'scheduled_watcher_expiry_after_manager_death_claimed':False,'leases':[{'owner':l.owner,'cleanup':l.cleanup_result} for l in leases],'source_hashes':{n:hashlib.sha256(pathlib.Path(__file__).with_name(n).read_bytes()).hexdigest() for n in ['linux_smoke.py','metadata_probe.py','lease.py','linux_inventory.py','cleanup.py','canary_preflight.py','canary_payload.py']}})
 if not passed:raise SystemExit(1)
if __name__=='__main__':main()
