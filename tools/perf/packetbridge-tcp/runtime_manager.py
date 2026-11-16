"""Same R3 canary/Lease lifetime, then exactly OLD/NEW public PacketBridge TCP."""
import argparse,hashlib,json,os,pathlib,secrets,signal,subprocess,sys,time
HERE=pathlib.Path(__file__).resolve().parent;sys.path.insert(0,str(HERE.parent/'f997-baseline'))
from lease import Lease,atomic
from canary_preflight import run_canary
from netgen_capture import strict_result

ap=argparse.ArgumentParser();ap.add_argument('--bundle',required=True);ap.add_argument('--manifest-sha256',required=True);ap.add_argument('--out',required=True);ap.add_argument('--job-start',required=True,type=float);a=ap.parse_args()
assert os.geteuid()==0 and sys.platform=='linux' and os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted'
run=os.environ['GITHUB_RUN_ID'];attempt=os.environ['GITHUB_RUN_ATTEMPT'];out=pathlib.Path(a.out).resolve();out.mkdir(mode=0o700);atomic(out/'JOB-OWNER.json',{'run_id':run,'attempt':attempt,'owner':'gh'+run+'a'+attempt+'-tcp-job-'+secrets.token_hex(6)})
bundle=pathlib.Path(a.bundle).resolve();raw=(bundle/'BINARY-MANIFEST.json').read_bytes();assert hashlib.sha256(raw).hexdigest()==a.manifest_sha256;m=json.loads(raw);pin=json.loads((HERE/'SOURCE.json').read_text())
assert m['base']==pin['base'] and m['tree']==pin['tree'] and m['lock_sha256']==pin['Cargo_lock_sha256'] and m['source_pre_post_clean']is True
assert m['cargo_profile_config']==pin['cargo_profile_config'] and m['profile']==pin['profile'] and m['cargo_jobs']==1 and m['build_run']==run
for arm,row in m['arms'].items():assert row['source_sha256']==pin['patched_source'][arm] and row['example_sha256']==pin['example_sha256']
for rel,row in m['files'].items():
 p=bundle/rel;assert p.resolve().is_relative_to(bundle) and not p.is_symlink() and p.stat().st_size==row['bytes'] and hashlib.file_digest(p.open('rb'),'sha256').hexdigest()==row['sha256']
assert m['files']['harness/run.py']['sha256']=='54aff15a8dbfc6eac6fe4c1ba7f153d449d5d16a0a1d40bda6ba2186ebb2aa20'
assert m['files']['harness/netns.sh']['sha256']=='09cd154e0e2ef2918584199e8a3d3ce2deb1bcaa0fb6b1e8ac0530966347b7ed'
assert m['files']['bin/sing-box']['sha256']=='fc9c6e6ab345f045b16a0ed10d1ff28d68e8e56e7749fca30738d1406e98d7b8'
cpus=sorted(os.sched_getaffinity(0))[:4];assert len(cpus)==4;rows=[];passed=False;preflight_passed=False
def interrupted(n,f):raise SystemExit(128+n)
signal.signal(signal.SIGTERM,interrupted);signal.signal(signal.SIGINT,interrupted)
try:
 with Lease(out/'preflight','preflight',cpus,a.job_start) as preflight:
  proof=run_canary(bundle,preflight.out,preflight.registry,preflight.env);assert proof['passed']is True
 assert preflight.cleanup_result['fresh_final_empty_proof']['passed']is True and not preflight.cleanup_result['oom_occurred']
 preflight_passed=True
 (out/'preflight-complete.actual.json').write_text(json.dumps({'actual_canary':proof,'actual_cleanup':preflight.cleanup_result},indent=2)+'\n')
 with Lease(out/'runtime','traffic',cpus,a.job_start) as lease:
  for i,arm in enumerate(['old','new']):
   assert time.monotonic()-a.job_start+240<1170,'insufficient case budget'
   capture=lease.out/(arm+'-netgen');capture.mkdir(mode=0o700)
   env={**lease.env,'PPVPN_REAL_NETGEN':str(bundle/'bin/netgen'),'PPVPN_NETGEN_SHA256':m['files']['bin/netgen']['sha256'],'PPVPN_PERF_CAPTURE_DIR':str(capture),'PPVPN_EXPECTED_STREAMS':'1'}
   argv=[sys.executable,str(HERE/'case.py'),'--bundle',str(bundle),'--arm',arm,'--work',str(lease.out/arm),'--cpus',','.join(map(str,cpus[:2])),'--server-cpus',','.join(map(str,cpus[2:]))]
   command=['systemd-run','--wait','--pipe','--unit',lease.units[i],'--slice',lease.d['slice'],'--property=RuntimeMaxSec=240s','--property=MemoryMax=2147483648','--property=MemorySwapMax=0','--property=CPUQuota=400%','--property=AllowedCPUs='+','.join(map(str,cpus)),'--property=TasksMax=256','--property=KillMode=control-group','--property=TimeoutStopSec=5s','--property=Environment=PPVPN_PERF_OWNER='+lease.owner]
   for k,v in env.items():command+=['--setenv='+k+'='+v]
   command+=argv;(lease.out/(arm+'-actual-systemd-invocation.json')).write_text(json.dumps(command,indent=2)+'\n')
   start=time.monotonic()
   with(lease.out/(arm+'-service.stdout.raw')).open('wb')as stdout,(lease.out/(arm+'-service.stderr.raw')).open('wb')as stderr:p=subprocess.run(command,stdout=stdout,stderr=stderr,timeout=250)
   row={'arm':arm,'actual_service_exit':p.returncode,'service_seconds':time.monotonic()-start};rows.append(row);(out/'actual-case-exits.json').write_text(json.dumps(rows,indent=2)+'\n');assert p.returncode==0
   case=json.loads((lease.out/arm/'CASE-RESULT.json').read_text());assert case['passed']is True and case['host_actual_exit']==0 and case['fresh_host_fd_tun_far_route_cleanup']is True and case['server_route_preserved']is True
   receipts=list(capture.glob('*.receipt.json'));assert len(receipts)==1;r=json.loads(receipts[0].read_text());assert r['validated']is True and r['actual_netgen_exit']==0 and not r['timed_out'] and r['binary_sha256']==m['files']['bin/netgen']['sha256']
   business=strict_result((capture/r['stdout_file']).read_bytes(),r['actual_netgen_exit'],1,r['argv']);assert r['argv'][r['argv'].index('-target')+1]=='10.'+str(int(lease.env['NETEM_NET'])+1)+'.0.1:9000'
   for stream in ['stdout','stderr']:assert hashlib.sha256((capture/r[stream+'_file']).read_bytes()).hexdigest()==r[stream+'_sha256']
   assert case['netgen']==business;row.update({'strict_netgen':business,'host_stats':case['host_stats'],'workload_host_measurement':case['load'],'routes_and_host_fd_freshly_clean':True});(out/'actual-case-exits.json').write_text(json.dumps(rows,indent=2)+'\n')
 passed=True
finally:
 (out/'MANAGED-RESULT.json').write_text(json.dumps({'passed':passed,'actual_linux_preflight_passed':preflight_passed,'cases':rows,'mode':'PUBLIC_PACKET_BRIDGE_TCP_HOST_ONLY_NOT_NATIVE_TUN','shipping_iOS_or_200Mbps_acceptance':False},indent=2)+'\n')
if not passed:raise SystemExit(1)
