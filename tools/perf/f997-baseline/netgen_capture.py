#!/usr/bin/env python3
"""Preserve exact netgen bulk output/exit; validate without changing harness."""
import hashlib,json,math,os,pathlib,subprocess,sys,time

def strict_result(raw,exit_code,streams,argv):
 if exit_code!=0:raise ValueError('actual netgen nonzero exit')
 if not raw or len(raw)>1048576:raise ValueError('missing/oversize stdout')
 def pairs(items):
  d={}
  for k,v in items:
   if k in d:raise ValueError('duplicate JSON key')
   d[k]=v
  return d
 def bad(x):raise ValueError('nonfinite JSON constant')
 r=json.loads(raw,object_pairs_hook=pairs,parse_constant=bad)
 if not isinstance(r,dict):raise ValueError('object required')
 if any(type(r.get(k)) is not int for k in ['ok','failed','corrupt','streams','bytes']):raise ValueError('missing/noninteger required field')
 if streams not in [1,4] or (r['ok'],r['failed'],r['corrupt'],r['streams'],r['bytes'])!=(streams,0,0,streams,67108864):raise ValueError('incomplete/corrupt/wrong-stream transfer')
 for key in ['seconds','mbps']:
  if type(r.get(key)) not in [float,int] or not math.isfinite(r[key]) or r[key]<=0:raise ValueError('invalid timing/rate')
 if not math.isclose(r['mbps'],67108864*8/r['seconds']/1e6,rel_tol=1e-8):raise ValueError('rate denominator mismatch')
 if 'errors' in r and r['errors']!=[]:raise ValueError('errors must be absent or empty array')
 def arg(key):
  if argv.count(key)!=1:raise ValueError('missing/duplicate '+key)
  return argv[argv.index(key)+1]
 if argv[0]!='bulk' or arg('-dir')!='down' or int(arg('-streams'))!=streams or int(arg('-bytes'))!=67108864//streams:raise ValueError('wrong workload arguments')
 return r

def main():
 args=sys.argv[1:];real=os.environ['PPVPN_REAL_NETGEN'];assert hashlib.sha256(pathlib.Path(real).read_bytes()).hexdigest()==os.environ['PPVPN_NETGEN_SHA256']
 if not args or args[0]!='bulk':os.execv(real,[real,*args])
 capture=pathlib.Path(os.environ['PPVPN_PERF_CAPTURE_DIR']);capture.mkdir(exist_ok=True);stem=str(time.time_ns());stdout=capture/(stem+'.stdout');stderr=capture/(stem+'.stderr');start=time.monotonic();timed_out=False
 with stdout.open('wb') as o,stderr.open('wb') as e:
  try:p=subprocess.run([real,*args],stdout=o,stderr=e,timeout=235);code=p.returncode
  except subprocess.TimeoutExpired:code=124;timed_out=True
 seconds=time.monotonic()-start;raw=stdout.read_bytes();error=None;result=None
 try:result=strict_result(raw,code,int(os.environ['PPVPN_EXPECTED_STREAMS']),args)
 except (ValueError,IndexError,UnicodeError) as ex:error=str(ex)
 rec={'schema':1,'argv':args,'actual_netgen_exit':code,'timed_out':timed_out,'validated':error is None,'validation_error':error,'subprocess_seconds':seconds,'netgen_result':result,'stdout_file':stdout.name,'stderr_file':stderr.name,'stdout_sha256':hashlib.sha256(raw).hexdigest(),'stderr_sha256':hashlib.sha256(stderr.read_bytes()).hexdigest(),'binary_sha256':os.environ['PPVPN_NETGEN_SHA256']}
 (capture/(stem+'.receipt.json')).write_text(json.dumps(rec,indent=2,allow_nan=False)+'\n')
 os.write(1,raw);os.write(2,stderr.read_bytes());raise SystemExit(code if code else (97 if error else 0))
if __name__=='__main__':main()
