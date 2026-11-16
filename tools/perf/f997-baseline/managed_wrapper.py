"""Preparation-only review candidate: bounded dedicated-GitHub Linux baseline."""
import argparse,hashlib,ipaddress,json,os,pathlib,re,secrets,signal,subprocess,sys,time
from cleanup import cleanup,cmd
PINS={'harness/run.py':'54aff15a8dbfc6eac6fe4c1ba7f153d449d5d16a0a1d40bda6ba2186ebb2aa20','harness/netns.sh':'09cd154e0e2ef2918584199e8a3d3ce2deb1bcaa0fb6b1e8ac0530966347b7ed','bin/sing-box':'fc9c6e6ab345f045b16a0ed10d1ff28d68e8e56e7749fca30738d1406e98d7b8'}
SOURCE='f99761138b23f267ebc7621057afe02018ab7c18'
def atomic_registry(path,data):
 temp=path.with_suffix('.new');temp.write_text(json.dumps(data,indent=2)+'\n');temp.chmod(0o600);os.replace(temp,path)
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--bundle',required=True);ap.add_argument('--manifest-sha256',required=True);ap.add_argument('--out',required=True);ap.add_argument('--job-start',type=float,required=True);a=ap.parse_args()
 assert os.geteuid()==0 and sys.platform=='linux'
 assert os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted'
 assert pathlib.Path('/sys/fs/cgroup/cgroup.controllers').is_file()
 run=os.environ['GITHUB_RUN_ID'];attempt=os.environ['GITHUB_RUN_ATTEMPT'];assert run.isdecimal() and attempt.isdecimal()
 out=pathlib.Path(a.out).resolve();out.mkdir(parents=True,exist_ok=False);os.chmod(out,0o700)
 from lease import atomic
 atomic(out/'JOB-OWNER.json',{'run_id':run,'attempt':attempt,'owner':'gh'+run+'a'+attempt+'-baseline-job-'+secrets.token_hex(6),'scope':'BASELINE_EVIDENCE_ROOT'})
 bundle=pathlib.Path(a.bundle).resolve();manifest=bundle/'BINARY-MANIFEST.json';raw=manifest.read_bytes();assert hashlib.sha256(raw).hexdigest()==a.manifest_sha256
 m=json.loads(raw);assert m['source']==SOURCE and m['tree']=='c7306832afbbdf77d78d21907756c95207136b94'
 assert type(m['build_provenance_actual_exit']) is int and m['build_provenance_actual_exit']==0 and m['netgen_source']==SOURCE and m['sail_source']==SOURCE
 assert m['cargo_lock_sha256']=='dfeaf3c14bcf23fb02157e2e103352eeb27a3c738fbc6124678f8e2d4b58c68a'
 assert m['netgen_sources']=={'main.go':'f591a5492b128f5ec858759723baab22f6305f9551f6a46c99ca5d9c33124f4d','go.mod':'951393dbe66eaec030e9f3899da5af8cdb78265f358d3fd0078faece146d03c4','cc_linux.go':'70f9b3827f16082fb8c3037446d543a5de962b6e0af4a4487504c3043225bb90','cc_other.go':'8eadebd6e4866306b6f1384e98c983070ee582016ab1fb3eb7ab097f744cb422'}
 front=json.loads((bundle/'FRONT-BUILD-GATE-RESULT.json').read_text());assert front['passed'] is True and front['actual_service_exit']==0 and front['source']==SOURCE
 assert front['cleanup']['fresh_final_empty_proof']['passed'] is True and not front['cleanup']['errors'] and not front['cleanup']['oom_occurred']
 provenance=bundle/'build-provenance.json';assert hashlib.sha256(provenance.read_bytes()).hexdigest()==m['build_provenance_sha256']
 actual=json.loads(provenance.read_text());assert actual['source']==SOURCE and actual['tree']==m['tree'] and actual['source_pre_post_clean'] is True and all(type(r['exit']) is int and r['exit']==0 for r in actual['stages'])
 for rel,pin in PINS.items():assert m['files'][rel]['sha256']==pin
 for rel in ['harness/run.py','harness/netns.sh','bin/sail','bin/netgen','bin/sing-box']:
  p=bundle/rel;assert p.resolve().is_relative_to(bundle) and not p.is_symlink() and p.is_file()
  assert p.stat().st_size==m['files'][rel]['bytes']
  pin=m['files'][rel]['sha256'];assert re.fullmatch('[0-9a-f]{64}',pin) and hashlib.sha256(p.read_bytes()).hexdigest()==pin
 available=sorted(os.sched_getaffinity(0));assert len(available)>=4,'runner needs four CPUs'
 cpus=available[:4];client=','.join(map(str,cpus[:2]));peer=','.join(map(str,cpus[2:]))
 def interrupted(signum,frame):raise SystemExit(128+signum)
 signal.signal(signal.SIGTERM,interrupted);signal.signal(signal.SIGINT,interrupted)
 from lease import Lease
 from canary_preflight import run_canary
 result_root=out;rows=[];passed=False;preflight_passed=False;runtime_started=False
 try:
  with Lease(result_root/'preflight','preflight',cpus,a.job_start) as preflight:
   proof=run_canary(bundle,preflight.out,preflight.registry,preflight.env)
   assert proof['passed'] is True and proof['run_id']==run and proof['owner']==preflight.owner
  empty=preflight.cleanup_result
  assert empty['fresh_final_empty_proof']['passed'] is True and empty['run_id']==run and not empty['errors'] and not empty['oom_occurred']
  final_preflight={'run_id':run,'owner':preflight.owner,'actual_modes':proof,'full_lease_cleanup':empty,'passed':True}
  (result_root/'preflight-complete.actual.json').write_text(json.dumps(final_preflight,indent=2)+'\n')
  preflight_passed=True
  # Hard gate: only a fresh fully reclaimed real Linux preflight reaches traffic.
  assert preflight_passed
  with Lease(result_root/'runtime','traffic',cpus,a.job_start) as lease:
   out=lease.out;env=lease.env;owner=lease.owner;units=lease.units;slice_name=lease.d['slice'];net=int(env['NETEM_NET']);runtime_started=True
   for i,(inbound,streams) in enumerate([('tun',1),('tun',4),('socks',1),('socks',4)]):
    assert time.monotonic()-a.job_start+240<1170,'insufficient remaining job budget; invalid incomplete matrix'
    args=[sys.executable,str(bundle/'harness/run.py'),'--work',str(out/('case-'+str(i))),'--sail',str(bundle/'bin/sail'),'--netgen',str(pathlib.Path(__file__).with_name('netgen_capture.py')),'--singbox',str(bundle/'bin/sing-box'),'--clients','sail-mobile','--protocols','trojan','--servers','sing-box','--inbound',inbound,'--only','baseline','--bulk-only','--rounds','1','--streams',str(streams),'--bulk-bytes',str(67108864//streams),'--client-set','relay.buffer_max_size=32','--client-set','inbound.max_connections=256','--benchmark-direction','down','--benchmark-same-target','--benchmark-keep-raw','--end-settle','2','--cpus',client,'--server-cpus',peer]
    command=['systemd-run','--wait','--pipe','--collect','--unit',units[i],'--slice',slice_name,'--property=RuntimeMaxSec=240s','--property=KillMode=control-group','--property=TimeoutStopSec=5s','--property=Environment=PPVPN_PERF_OWNER='+owner]
    for k in ['NETEM_NS','NETEM_NET','NETEM_LOCK_DIR','NETEM_LOCK_OWNER']:command+=['--setenv='+k+'='+env[k]]
    capture=out/('case-'+str(i)+'-netgen');capture.mkdir()
    for k,v in {'PPVPN_REAL_NETGEN':str(bundle/'bin/netgen'),'PPVPN_NETGEN_SHA256':m['files']['bin/netgen']['sha256'],'PPVPN_PERF_CAPTURE_DIR':str(capture),'PPVPN_EXPECTED_STREAMS':str(streams)}.items():command+=['--setenv='+k+'='+v]
    start=time.monotonic()
    with (out/('case-'+str(i)+'.original.log')).open('wb') as log:p=subprocess.run(command+args,stdout=log,stderr=subprocess.STDOUT,timeout=250)
    rows.append({'case':i,'inbound':inbound,'streams':streams,'application_bytes':67108864,'direction':'down','effective_mobile_profile':{'profile':'mobile','relay_buffer_initial_KiB':8,'relay_buffer_max_KiB':32,'inbound_max_connections':256,'binding':'exact F997 profile source plus actual two --client-set arguments'},'background_flows':'no additional generated workload; actual TCP namespace samples retained in raw.jsonl.gz','cap_hit':'not independently instrumented; do not infer 256-concurrency acceptance from 1/4 bulk streams','command':args,'exit':p.returncode,'seconds':time.monotonic()-start});(out/'actual-case-exits.json').write_text(json.dumps(rows,indent=2)+'\n')
    assert p.returncode==0,'case failure/timeout is not low throughput success'
    receipts=list(capture.glob('*.receipt.json'));assert len(receipts)==1,'missing/duplicate real netgen execution'
    observed=json.loads(receipts[0].read_text());assert observed['validated'] is True and observed['actual_netgen_exit']==0 and not observed['timed_out']
    from netgen_capture import strict_result
    validated=strict_result((capture/observed['stdout_file']).read_bytes(),observed['actual_netgen_exit'],streams,observed['argv'])
    assert observed['binary_sha256']==m['files']['bin/netgen']['sha256']
    for stream in ['stdout','stderr']:assert hashlib.sha256((capture/observed[stream+'_file']).read_bytes()).hexdigest()==observed[stream+'_sha256']
    assert observed['argv'][observed['argv'].index('-target')+1]=='10.'+str(net+1)+'.0.1:9000'
    summaries=list((out/('case-'+str(i))).glob('results/*/summary.json'));assert len(summaries)==1
    summary=json.loads(summaries[0].read_text());assert summary['schema']==1 and len(summary['runs'])==1
    runrow=summary['runs'][0];assert not runrow['failures'] and len(runrow['records'])==1
    record=runrow['records'][0];assert record['scenario']=='baseline' and record['workload']=='bulk_down' and record['client_alive'] is True
    for key in ['ok','failed','corrupt','streams','bytes','seconds','mbps']:assert record['result'][key]==validated[key]
    rows[-1]['strict_real_netgen_validated']=True;rows[-1]['netgen_seconds']=validated['seconds'];rows[-1]['netgen_mbps']=validated['mbps'];rows[-1]['subprocess_seconds']=observed['subprocess_seconds']
    (out/'actual-case-exits.json').write_text(json.dumps(rows,indent=2)+'\n')
  passed=True
 finally:
  (result_root/'MANAGED-RESULT.json').write_text(json.dumps({'passed':passed,'actual_linux_preflight_passed':preflight_passed,'runtime_started':runtime_started,'cases':rows,'source':SOURCE,'hardware':'dedicated Linux native TUN/SOCKS; no iOS Swift HostPacketIo equivalence'},indent=2)+'\n')
 if not passed:raise SystemExit(1)
if __name__=='__main__':main()
