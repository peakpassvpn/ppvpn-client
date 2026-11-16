"""Stage A executable resource wrapper; actual invocation is recorded."""
import argparse,hashlib,json,os,pathlib,pwd,signal,subprocess,sys,time
from lease import Lease

def main():
 ap=argparse.ArgumentParser();ap.add_argument('--source',required=True);ap.add_argument('--artifact-output',required=True);ap.add_argument('--resource-output',required=True);ap.add_argument('--job-start',type=float,required=True);ap.add_argument('--singbox-archive',required=True);ap.add_argument('--reviewed-harness',required=True);a=ap.parse_args()
 assert os.geteuid()==0 and sys.platform=='linux' and os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted'
 cpus=sorted(os.sched_getaffinity(0));assert len(cpus)>=4;cpus=cpus[:4]
 uid=int(os.environ['SUDO_UID']);assert uid!=0;user=pwd.getpwuid(uid)
 root=pathlib.Path(a.source).resolve();artifact=pathlib.Path(a.artifact_output).resolve();assert not artifact.exists() and not artifact.is_relative_to(root)
 resource=pathlib.Path(a.resource_output).resolve();assert not resource.exists();resource.parent.mkdir(exist_ok=True,parents=True)
 def interrupted(n,f):raise SystemExit(128+n)
 signal.signal(signal.SIGTERM,interrupted);signal.signal(signal.SIGINT,interrupted)
 passed=False;lease=None
 try:
  with Lease(resource,'build',cpus,a.job_start,memory=8589934592,tasks=512,cleanup_at=1770,network=False) as lease:
   remaining=int(1770-(time.monotonic()-a.job_start));assert remaining>30
   env={'HOME':user.pw_dir,'PATH':os.environ['PATH'],'F997_SOURCE_CHECKOUT':str(root),'F997_BINARY_OUTPUT':str(artifact),'F997_SINGBOX_ARCHIVE':str(pathlib.Path(a.singbox_archive).resolve()),'F997_REVIEWED_HARNESS':str(pathlib.Path(a.reviewed_harness).resolve()),'GITHUB_RUN_ID':os.environ['GITHUB_RUN_ID'],'GITHUB_JOB':os.environ['GITHUB_JOB']}
   for key in ['GOCACHE','GOMODCACHE','GOTMPDIR','GOBIN','GOTOOLCHAIN','CARGO_TARGET_DIR','RUSTUP_HOME','CARGO_HOME']:
    if key in os.environ:env[key]=os.environ[key]
   env.update({k:v for k,v in os.environ.items() if k.startswith(('BORING_','BTLS_'))})
   command=['systemd-run','--wait','--pipe','--unit',lease.build_unit,'--slice',lease.d['slice'],'--uid',str(uid),'--working-directory',str(root),'--property=RuntimeMaxSec='+str(remaining)+'s','--property=MemoryMax=8589934592','--property=MemorySwapMax=0','--property=CPUQuota=400%','--property=AllowedCPUs='+','.join(map(str,cpus)),'--property=TasksMax=512','--property=KillMode=control-group','--property=TimeoutStopSec=2s','--property=Environment=PPVPN_PERF_OWNER='+lease.owner]
   for k,v in env.items():command+=['--setenv='+k+'='+v]
   command +=[sys.executable,str(pathlib.Path(__file__).with_name('build_exact_f997.py'))]
   (resource/'actual-systemd-invocation.json').write_text(json.dumps({'command':command,'job_max_seconds':1800,'payload_runtime_max_seconds':remaining,'source':'f99761138b23f267ebc7621057afe02018ab7c18'},indent=2)+'\n')
   with (resource/'actual-build-service.original.log').open('wb') as f:p=subprocess.run(command,stdout=f,stderr=subprocess.STDOUT,timeout=remaining+5)
   (resource/'actual-build-service.exit.json').write_text(json.dumps({'exit':p.returncode})+'\n');assert p.returncode==0
  passed=True
 finally:
  result={'passed':passed,'source':'f99761138b23f267ebc7621057afe02018ab7c18','run_id':os.environ['GITHUB_RUN_ID'],'resource_owner':lease.owner if lease else None,'actual_service_exit':0 if passed else None,'cleanup':lease.cleanup_result if lease else None,'resource_wrapper_sha256':hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()}
  if artifact.is_dir():(artifact/'FRONT-BUILD-GATE-RESULT.json').write_text(json.dumps(result,indent=2)+'\n')
 if not passed:raise SystemExit(1)
if __name__=='__main__':main()
