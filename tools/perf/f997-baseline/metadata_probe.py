"""No compilation or generated network load: actual unprivileged unit metadata."""
import json,os,pathlib,shutil,subprocess
cg=pathlib.Path('/sys/fs/cgroup')/pathlib.Path('/proc/self/cgroup').read_text().strip().split('::',1)[1].lstrip('/')
r={'uid':os.geteuid(),'affinity':sorted(os.sched_getaffinity(0)),'PATH':os.environ['PATH'],'cgroup':str(cg),'cgroup_files':{n:(cg/n).read_text() for n in ['memory.max','memory.swap.max','cpu.max','pids.max','cpuset.cpus.effective']},'injected_Go_environment':{k:os.environ.get(k) for k in ['GOCACHE','GOMODCACHE','GOTMPDIR','GOBIN']}}
for name,args in [('Go',['go','version']),('Go_paths',['go','env','GOCACHE','GOMODCACHE','GOTMPDIR','GOBIN']),('Rust',['rustc','-vV'])]:
 if shutil.which(args[0]):
  p=subprocess.run(args,capture_output=True,text=True,timeout=5);r[name]={'exit':p.returncode,'stdout':p.stdout,'stderr':p.stderr}
 else:r[name]=None
print(json.dumps(r))
