"""One bounded PacketBridge build or measurement service; proven Lease unchanged."""
import argparse,json,os,pathlib,pwd,signal,subprocess,sys,time
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parent.parent/'f997-baseline'))
from lease import Lease

ap=argparse.ArgumentParser();ap.add_argument('phase',choices=['build']);ap.add_argument('--source',required=True);ap.add_argument('--output',required=True);ap.add_argument('--resource',required=True);ap.add_argument('--job-start',required=True,type=float);ap.add_argument('--singbox-archive',required=True);a=ap.parse_args()
assert os.geteuid()==0 and sys.platform=='linux' and os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted'
cpus=sorted(os.sched_getaffinity(0))[:4];assert len(cpus)==4
uid=int(os.environ['SUDO_UID']);assert uid>0;user=pwd.getpwuid(uid)
root=pathlib.Path(a.source).resolve();out=pathlib.Path(a.output).resolve();resource=pathlib.Path(a.resource).resolve();assert not out.exists() and not resource.exists()
build=a.phase=='build';memory=8589934592 if build else 2147483648;tasks=512 if build else 256;deadline=1770 if build else 1170
def interrupted(n,f):raise SystemExit(128+n)
signal.signal(signal.SIGTERM,interrupted);signal.signal(signal.SIGINT,interrupted)
with Lease(resource,a.phase,cpus,a.job_start,memory=memory,tasks=tasks,cleanup_at=deadline,network=False) as lease:
 remaining=int(deadline-(time.monotonic()-a.job_start));assert remaining>30
 env={'HOME':user.pw_dir,'PATH':os.environ['PATH'],'PB_SOURCE':str(root),'PB_OUTPUT':str(out),'PB_PHASE':a.phase,'PB_SINGBOX_ARCHIVE':str(pathlib.Path(a.singbox_archive).resolve()),'GITHUB_RUN_ID':os.environ['GITHUB_RUN_ID'],'GITHUB_JOB':os.environ['GITHUB_JOB']}
 for k in ['RUSTUP_HOME','CARGO_HOME','GOCACHE','GOMODCACHE','GOTMPDIR','GOBIN','GOTOOLCHAIN']:
  if k in os.environ:env[k]=os.environ[k]
 env.update({k:v for k,v in os.environ.items() if k.startswith(('BORING_','BTLS_'))})
 command=['systemd-run','--wait','--pipe','--unit',lease.build_unit,'--slice',lease.d['slice'],'--uid',str(uid),'--working-directory',str(root),'--property=RuntimeMaxSec='+str(remaining)+'s','--property=MemoryMax='+str(memory),'--property=MemorySwapMax=0','--property=CPUQuota=400%','--property=AllowedCPUs='+','.join(map(str,cpus)),'--property=TasksMax='+str(tasks),'--property=KillMode=control-group','--property=TimeoutStopSec=2s','--property=Environment=PPVPN_PERF_OWNER='+lease.owner]
 for k,v in env.items():command+=['--setenv='+k+'='+v]
 command += [sys.executable,str(pathlib.Path(__file__).with_name('build_payload.py'))]
 (resource/'actual-systemd-invocation.json').write_text(json.dumps({'command':command,'remaining_seconds':remaining,'phase':a.phase},indent=2)+'\n')
 with (resource/'actual-service.stdout.raw').open('wb') as stdout,(resource/'actual-service.stderr.raw').open('wb') as stderr:r=subprocess.run(command,stdout=stdout,stderr=stderr,timeout=remaining+5)
 (resource/'actual-service.exit.json').write_text(json.dumps({'exit':r.returncode})+'\n');assert r.returncode==0
