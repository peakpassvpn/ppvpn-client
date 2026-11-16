import json,pathlib,re

def cargo_example_artifact(text,checkout,cargo_exit):
 assert type(cargo_exit) is int and cargo_exit==0,'Actual cargo exit must be zero'
 checkout=pathlib.Path(checkout)
 package='path+'+(checkout/'sail').as_uri()+'#0.24.0'
 expected_profile={'opt_level':'3','debuginfo':0,'debug_assertions':False,'overflow_checks':False,'test':False}
 # Cargo -vv interleaves bracket-prefixed build-script text with JSON records.
 # Keep the original stream; accept only that known non-JSON envelope.
 executables=[];finished=[];verbose_lines=0;json_records=0
 for number,line in enumerate(text.splitlines(),1):
  try:obj=json.loads(line)
  except json.JSONDecodeError:
   if re.fullmatch(r'\[[^\]\r\n]+\](?: .*)?',line):verbose_lines+=1;continue
   raise ValueError('Unrecognized Cargo stdout record at line '+str(number))
  assert isinstance(obj,dict);json_records+=1
  if obj.get('reason')=='build-finished':finished.append(obj.get('success'))
  assert not (obj.get('reason')=='compiler-message' and obj.get('message',{}).get('level')=='error'),'Cargo reported a compiler error'
  target=obj.get('target',{});profile=obj.get('profile',{})
  profile_ok=isinstance(profile,dict) and set(profile)==set(expected_profile) and all(type(profile[k]) is type(v) and profile[k]==v for k,v in expected_profile.items())
  if obj.get('reason')=='compiler-artifact' and obj.get('package_id')==package and obj.get('manifest_path')==str(checkout/'sail/Cargo.toml') and target.get('name')=='packet_bridge_tcp_bench' and target.get('kind')==['example'] and target.get('crate_types')==['bin'] and target.get('src_path')==str(checkout/'sail/examples/packet_bridge_tcp_bench.rs') and profile_ok and obj.get('executable'):
   executable=obj['executable'];assert isinstance(executable,str)
   path=pathlib.Path(executable)
   assert path.is_absolute() and path==checkout/'target/dist-mobile/examples/packet_bridge_tcp_bench' and executable in obj.get('filenames',[]),'Unexpected native example executable path'
   executables.append(executable)
 assert len(finished)==1 and finished[0] is True,'Require exactly one successful Cargo build-finished record'
 assert len(executables)==1,'Require exactly one actual sail example executable'
 return executables[0],{'verbose_build_script_lines':verbose_lines,'JSON_records':json_records,'build_finished_success':True,'actual_cargo_exit':cargo_exit,'executable':executables[0]}
