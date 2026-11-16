"""OWN CANARY ONLY: bounded detached descendants; never generates traffic."""
import json,os,pathlib,subprocess,sys,time
netns,ns,record=sys.argv[1:];subprocess.run([netns,'up'],check=True)
# Both a session-detached child and its session-detached grandchild remain
# within the one transient canary cgroup and client namespace.
child='''import json,os,pathlib,time
os.setsid()
p=os.fork()
if p==0:
 os.setsid()
 time.sleep(30)
else:
 tmp=pathlib.Path(RECORD+'.new');tmp.write_text(json.dumps({'child':os.getpid(),'grandchild':p})+'\\n');os.replace(tmp,RECORD)
 time.sleep(30)
'''.replace('RECORD',repr(record))
subprocess.Popen(['ip','netns','exec',ns,sys.executable,'-c',child])
time.sleep(30)
