"""Alternative public PacketBridge host only; frozen NativeTun harness stays intact."""
import argparse,gzip,hashlib,importlib.util,json,os,pathlib,re,signal,subprocess,time,types

def strict_json(raw):
 def pairs(items):
  d={}
  for k,v in items:
   if k in d:raise ValueError('duplicate JSON field')
   d[k]=v
  return d
 def nonfinite(x):raise ValueError('nonfinite JSON')
 return json.loads(raw,object_pairs_hook=pairs,parse_constant=nonfinite)

def server_identity(rows,tun):
 assert isinstance(rows,list) and len(rows)==1
 r=rows[0];assert isinstance(r.get('dev'),str) and r['dev']!=tun
 return {k:r.get(k)for k in ['dev','gateway','prefsrc']}

def ready_contract(r,tun):
 assert isinstance(r,dict) and r.get('ready') is True
 assert type(r.get('pid')) is int and r['pid']>1
 assert r.get('tun')==tun and r.get('profile')=='mobile'
 for k,v in [('workers',2),('queue_capacity',64),('mtu',9000),('session_cap',256)]:assert type(r.get(k))is int and r[k]==v
 assert type(r.get('relay_kib'))is list and all(type(x)is int for x in r['relay_kib']) and r['relay_kib']==[8,32]
 return r['pid']

def stats_contract(r,pid):
 assert r.get('pid')==pid and r.get('success') is True
 for k in ['pump_rx_ok','pump_tx_ok','detach_clean','stop_ok']:assert r.get(k)is True
 for side in ['uplink','downlink']:
  c=r.get(side);assert isinstance(c,dict)
  for k in ['packets','bytes','accepted_packets','accepted_bytes']:assert type(c.get(k))is int and c[k]>0
  assert c['accepted_packets']<=c['packets'] and c['accepted_bytes']<=c['bytes']
 assert 'native_ledger'in r and r['native_ledger'] is None

def main():
 ap=argparse.ArgumentParser();ap.add_argument('--bundle',required=True);ap.add_argument('--arm',choices=['old','new'],required=True);ap.add_argument('--work',required=True);ap.add_argument('--cpus',required=True);ap.add_argument('--server-cpus',required=True);a=ap.parse_args()
 assert os.geteuid()==0 and os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted'
 bundle=pathlib.Path(a.bundle).resolve();work=pathlib.Path(a.work).resolve();work.mkdir(mode=0o700);os.umask(0o077)
 for p in [bundle,work]:assert re.fullmatch(r'[A-Za-z0-9_/.-]+',str(p)),'Frozen shell helper requires ordinary managed paths'
 spec=importlib.util.spec_from_file_location('frozen_f997_harness',bundle/'harness/run.py');h=importlib.util.module_from_spec(spec);spec.loader.exec_module(h)
 h.CPUS[h.CLIENT_NS]=a.cpus;h.CPUS[h.SERVER_NS]=a.server_cpus
 manifest=strict_json((bundle/'BINARY-MANIFEST.json').read_bytes());example=bundle/manifest['arms'][a.arm]['binary'];assert hashlib.file_digest(example.open('rb'),'sha256').hexdigest()==manifest['arms'][a.arm]['binary_sha256']
 args=types.SimpleNamespace(work=str(work),out=str(work/'results'),sail=str(example),singbox=str(bundle/'bin/sing-box'),netgen=os.environ['PPVPN_REAL_NETGEN'],inbound='tun',server_profile='server')
 run=h.Run(args,'trojan','sail-mobile','sing-box');tun='pbh'+a.arm;ready=work/'host-ready.json';stop=work/'host-stop';stats=work/'host-stats.json';assert not any(p.exists() for p in [ready,stop,stats])
 report={'arm':a.arm,'passed':False,'mode':'PUBLIC_TUN_PACKET_FLOW_HOST_TUN_NOT_NATIVE_TUN','namespace':h.CLIENT_NS,'tun':tun,'errors':[],'cleanup_errors':[],'native_ledger':None,'native_perflow':None};host=None;sampler=None;pid=None;ns_up=False;route_added=False;load_started=False
 def record(name,argv,check=True):
  r=subprocess.run(argv,capture_output=True,timeout=10);(work/(name+'.stdout.raw')).write_bytes(r.stdout);(work/(name+'.stderr.raw')).write_bytes(r.stderr);(work/(name+'.exit.json')).write_text(json.dumps({'argv':argv,'exit':r.returncode})+'\n')
  if check:assert r.returncode==0,name+' failed'
  return r
 def ipjson(name,*args):return strict_json(record(name,['ip','-n',h.CLIENT_NS,'-j',*args]).stdout)
 def sample_process(label):
  assert pid and pathlib.Path('/proc',str(pid),'exe').resolve()==example
  for n in ['status','stat','schedstat','cgroup']:(work/(label+'.proc-'+n+'.raw')).write_bytes(pathlib.Path('/proc',str(pid),n).read_bytes())
  return sampler.one()
 def interrupted(n,f):raise SystemExit(128+n)
 signal.signal(signal.SIGTERM,interrupted);signal.signal(signal.SIGINT,interrupted)
 try:
  record('certificate',['openssl','req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:prime256v1','-nodes','-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost','-days','30','-keyout',str(work/'key.pem'),'-out',str(work/'cert.pem')])
  h.netns('up');ns_up=True;h.netns('clear')
  before=server_identity(ipjson('server-route-before','route','get',h.SERVER_ADDR),tun);report['server_route_identity']=before
  assert ipjson('far-route-before','route','show','exact',h.FAR_ADDR+'/32')==[]
  assert all(r['ifname']!=tun for r in ipjson('links-before','link','show'))
  run.start_server();assert all(p.alive() for p in run.procs.values())
  listens=record('server-ready-listeners',['ip','netns','exec',h.SERVER_NS,'ss','-ltnH']).stdout.decode();assert all(any(re.search(r':'+port+r'\s',l)for l in listens.splitlines())for port in ['8443','9000'])
  cfg=h.client_config('trojan','socks');cfg['inbounds'][0]['type']='mixed';assert len(cfg['outbounds'])==1 and cfg['outbounds'][0]['server']==h.SERVER_ADDR and cfg['outbounds'][0]['type']=='trojan'
  config=work/'client.json';config.write_text(json.dumps(cfg,indent=2)+'\n')
  argv=[str(example),'--config',str(config),'--tunname',tun,'--readyfile',str(ready),'--stopfile',str(stop),'--statsfile',str(stats)]
  (work/'host-argv.json').write_text(json.dumps(['ip','netns','exec',h.CLIENT_NS,'taskset','-c',a.cpus,*argv],indent=2)+'\n')
  host=h.Proc('packetbridge-host',h.CLIENT_NS,' '.join(argv),str(work/'host.original.log'));deadline=time.monotonic()+30;r=None
  while time.monotonic()<deadline:
   assert host.alive(),'host ended before READY'
   if ready.exists():
    try:r=strict_json(ready.read_bytes());break
    except (ValueError,UnicodeError):pass
   time.sleep(.02)
  assert r is not None,'host READY timeout';pid=ready_contract(r,tun);report['ready']=r
  assert pathlib.Path('/proc',str(pid),'exe').resolve()==example
  assert pathlib.Path('/proc',str(pid),'cgroup').read_text()==pathlib.Path('/proc/self/cgroup').read_text()
  assert os.stat('/proc/'+str(pid)+'/ns/net').st_ino==os.stat('/var/run/netns/'+h.CLIENT_NS).st_ino
  link=ipjson('host-tun-ready','-d','link','show','dev',tun);assert len(link)==1 and link[0]['mtu']==9000 and link[0]['linkinfo']['info_kind']=='tun'
  info=link[0]['linkinfo'].get('info_data',{});assert info.get('pi')is False and info.get('vnet_hdr')is False
  record('far-route-add',['ip','-n',h.CLIENT_NS,'route','add',h.FAR_ADDR+'/32','dev',tun,'src','172.19.0.1']);route_added=True
  far=ipjson('far-route-capture','route','get',h.FAR_ADDR);assert len(far)==1 and far[0]['dev']==tun and far[0].get('prefsrc')=='172.19.0.1'
  assert server_identity(ipjson('server-route-after','route','get',h.SERVER_ADDR),tun)==before
  proxy=types.SimpleNamespace(pid=lambda:pid);sampler=h.Sampler(lambda:proxy,interval=.05)
  before_sample=sample_process('load-before');assert sampler.t.is_alive();mark=sampler.mark();started=time.monotonic();load_started=True
  capture=pathlib.Path(os.environ['PPVPN_PERF_CAPTURE_DIR']);wrapper=pathlib.Path(__file__).parent.parent/'f997-baseline/netgen_capture.py'
  load=['ip','netns','exec',h.CLIENT_NS,'taskset','-c',a.cpus,'python3',str(wrapper),'bulk','-target',h.FAR_TARGET,'-streams','1','-bytes','67108864','-dir','down']
  (work/'load-argv.json').write_text(json.dumps(load,indent=2)+'\n')
  timed_out=False
  with (work/'netgen.stdout.raw').open('wb') as stdout,(work/'netgen.stderr.raw').open('wb')as stderr:
   result=subprocess.Popen(load,stdout=stdout,stderr=stderr,start_new_session=True)
   try:result.wait(timeout=150)
   except subprocess.TimeoutExpired:
    timed_out=True;os.killpg(result.pid,signal.SIGTERM)
    try:result.wait(timeout=3)
    except subprocess.TimeoutExpired:os.killpg(result.pid,signal.SIGKILL);result.wait()
  (work/'netgen-process.exit.json').write_text(json.dumps({'argv':load,'actual_exit':result.returncode,'timed_out':timed_out})+'\n')
  elapsed=time.monotonic()-started;assert sampler.t.is_alive();after_sample=sample_process('load-after');samples=[before_sample,*sampler.since(mark),after_sample]
  with gzip.open(work/'workload-samples.original.jsonl.gz','wt')as f:
   for item in samples:f.write(json.dumps(item)+'\n')
  report['load']={'actual_exit':result.returncode,'subprocess_seconds':elapsed,'CPU_ticks_seconds':(after_sample['cpu_ticks']-before_sample['cpu_ticks'])/os.sysconf('SC_CLK_TCK'),'thread_sched_CPU_seconds':(after_sample['cpu_ns']-before_sample['cpu_ns'])/1e9,'sampled_process_RSS_peak_KiB':max(x['rss_kb']for x in samples if'rss_kb'in x),'sampled_FD_peak':max(x['fds']for x in samples if'fds'in x),'samples':len(samples),'CPU_scope':'host process over actual netgen window; includes deliberate all-packet-byte hashing; no compiler CPU','RSS_scope':'sampled window including endpoints, not guaranteed transient peak or NE bound'}
  assert result.returncode==0 and not timed_out,'netgen nonzero exit/timeout'
  receipts=list(capture.glob('*.receipt.json'));assert len(receipts)==1
  receipt=strict_json(receipts[0].read_bytes());assert receipt['validated']is True and receipt['actual_netgen_exit']==0 and not receipt['timed_out']
  from_import=importlib.util.spec_from_file_location('strict_real_netgen',wrapper);module=importlib.util.module_from_spec(from_import);from_import.loader.exec_module(module)
  business=module.strict_result((capture/receipt['stdout_file']).read_bytes(),receipt['actual_netgen_exit'],1,receipt['argv']);assert receipt['argv'][receipt['argv'].index('-target')+1]==h.FAR_TARGET
  report['netgen']=business;report['business_integrity']='Exact netgen binary/internal payload verifier,actual exit0,corrupt0,failed0,67108864B; raw host IP hashes are not application hashes'
 except BaseException as ex:
  report['errors'].append(type(ex).__name__+': '+str(ex))
 finally:
  if sampler:sampler.stop()
  if host:
   try:
    stop.touch(exist_ok=False);code=host.p.wait(timeout=35);report['host_actual_exit']=code;assert code==0
    value=strict_json(stats.read_bytes());report['host_stats']=value;stats_contract(value,pid)
   except BaseException as ex:report['cleanup_errors'].append('host stop/stats: '+type(ex).__name__+': '+str(ex))
   finally:host.stop()
  if ns_up:
   try:
    links=ipjson('links-after-stop','link','show');assert all(r['ifname']!=tun for r in links),'host TUN still exists'
    routes=ipjson('far-route-after-stop','route','show','exact',h.FAR_ADDR+'/32')
    if routes:
     assert route_added and len(routes)==1 and routes[0].get('dev')==tun
     record('owned-far-route-delete',['ip','-n',h.CLIENT_NS,'route','del',h.FAR_ADDR+'/32','dev',tun])
    assert ipjson('far-route-final','route','show','exact',h.FAR_ADDR+'/32')==[]
    assert server_identity(ipjson('server-route-final','route','get',h.SERVER_ADDR),tun)==before
    assert not pid or not pathlib.Path('/proc',str(pid)).exists(),'host PID not reclaimed'
    report['fresh_host_fd_tun_far_route_cleanup']=True;report['server_route_preserved']=True
   except BaseException as ex:report['cleanup_errors'].append('routes/FD: '+type(ex).__name__+': '+str(ex))
  for proc in run.procs.values():proc.stop()
  if ns_up:
   try:h.netns('down')
   except BaseException as ex:report['cleanup_errors'].append('namespace: '+str(ex))
  report['passed']=load_started and not report['errors'] and not report['cleanup_errors'] and report.get('fresh_host_fd_tun_far_route_cleanup')is True and report.get('host_actual_exit')==0 and 'netgen'in report
  (work/'CASE-RESULT.json').write_text(json.dumps(report,indent=2)+'\n')
 if not report['passed']:raise SystemExit(1)

if __name__=='__main__':main()
