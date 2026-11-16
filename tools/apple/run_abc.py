"""Exact source-owned failed A/B commands; C original actual evidence retained; bounded owned process groups."""
import hashlib,json,os,pathlib,signal,subprocess,sys,time,shutil
root=pathlib.Path(os.environ['ABC_SOURCE']);out=pathlib.Path(os.environ['RUNNER_TEMP'])/'abc-output';begin=time.monotonic();rows=[]
commands=[('A',['python3','-B','apps/flutter/checks/ios_packet_ffi_bench.py','--legacy-original-zip',os.environ['LEGACY_ORIGINAL_ZIP'],'--output',str(out/'A')],2160),('B',['python3','-B','apps/flutter/checks/ios_history_export_test.py','--output',str(out/'B')],180)]
for name,argv,cap in commands:
 remaining=2580-(time.monotonic()-begin);limit=min(cap,remaining)
 if limit<=0:rows.append({'component':name,'status':'INCOMPLETE_JOB_BUDGET'});break
 start=time.monotonic();timed_out=False
 with (out/(name+'.original.stdout')).open('wb') as stdout,(out/(name+'.original.stderr')).open('wb') as stderr:
  p=subprocess.Popen(argv,cwd=root,stdout=stdout,stderr=stderr,start_new_session=True)
  try:code=p.wait(timeout=limit)
  except subprocess.TimeoutExpired:
   timed_out=True;os.killpg(p.pid,signal.SIGTERM)
   try:p.wait(timeout=3)
   except subprocess.TimeoutExpired:os.killpg(p.pid,signal.SIGKILL);p.wait()
   code=p.returncode
 row={'component':name,'command':argv,'exit':code,'timed_out':timed_out,'seconds':time.monotonic()-start,'process_group':p.pid,'native_subcommand_originals':'Source-owned per-command stdout/stderr/exit files retained where completed; timeout is incomplete, never PASS'};rows.append(row);(out/'component-results.json').write_text(json.dumps(rows,indent=2)+'\n');print(json.dumps(row),flush=True)
 # Keep actual libraries, but never upload reproducible Cargo target caches.
 if name=='A':
  for mode in ['legacy','event']:
   a=out/'A'/mode;lib=a/'cargo-target/mobile/libppvpn_tunnel.a'
   if lib.is_file():shutil.copy2(lib,a/'actual-libppvpn_tunnel.a')
(out/'final-results.json').write_text(json.dumps({'components':rows,'all_AB_commands_passed':len(rows)==2 and all(r.get('exit')==0 and not r.get('timed_out') for r in rows),'C_reused_actual_QA_sha256':'76f532ee9133e0d35b023707de39b6a4cacd4fc144599d112ed33342e6ca751c','App_archive':False,'device_actions':0},indent=2)+'\n')
raise SystemExit(0 if len(rows)==2 and all(r.get('exit')==0 and not r.get('timed_out') for r in rows) else 1)
