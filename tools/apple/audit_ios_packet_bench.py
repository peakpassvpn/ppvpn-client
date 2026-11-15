"""Execute frozen bench unchanged; record actual compiler/program return codes."""
import hashlib,json,os,pathlib,runpy,shutil,subprocess,sys,time
root=pathlib.Path(sys.argv[1]);audit=pathlib.Path(sys.argv[2]);audit.mkdir()
original_run=subprocess.run;index=0
def run(*args,**kwargs):
    global index
    command=args[0] if args else kwargs.get('args')
    observed=isinstance(command,(list,tuple)) and pathlib.Path(str(command[0])).name in ['swiftc','provider-perf']
    if not observed:return original_run(*args,**kwargs)
    index+=1;number=index;started=time.monotonic();row={'index':number,'command':[str(x) for x in command]}
    if pathlib.Path(str(command[0])).name=='swiftc':
        source=next(pathlib.Path(x) for x in command if str(x).endswith('.swift'))
        row['actual_composite_source_sha256']=hashlib.sha256(source.read_bytes()).hexdigest()
        shutil.copyfile(source,audit/f'{number}-actual-compiled.swift')
    else:row['actual_executable_sha256']=hashlib.sha256(pathlib.Path(command[0]).read_bytes()).hexdigest()
    try:
        result=original_run(*args,**kwargs);row['exit']=result.returncode
        for name in ['stdout','stderr']:
            raw=getattr(result,name) or b''
            if isinstance(raw,str):raw=raw.encode()
            (audit/f'{number}-{name}.raw').write_bytes(raw)
        return result
    except BaseException as error:
        row['exception_class']=type(error).__name__;raise
    finally:
        row['seconds']=time.monotonic()-started
        with (audit/'actual-command-exits.jsonl').open('a') as f:f.write(json.dumps(row)+'\n')
subprocess.run=run
script=root/'apps/flutter/checks/ios_packet_throughput.py'
sys.argv=[str(script),'--baseline','89fe2df881f665b9d4cd9cfebdcf5a7dabf3236c','--candidate','8336089829aaa344889c7400847b4512bbf100b9','--output','/tmp/ppvpn-ios-packet-ab-aca7d482']
runpy.run_path(str(script),run_name='__main__')
