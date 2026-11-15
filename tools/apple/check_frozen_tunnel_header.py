"""Validate actual preprocessed frozen C header against its exact export list."""
import importlib.util,json,pathlib,sys
root=pathlib.Path(sys.argv[1]);script=root/'apps/flutter/scripts/check-ios-exports.py'
spec=importlib.util.spec_from_file_location('contract',script);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
names=m.tunnel_contract(root/'crates/tunnel-host/include/ppvpn_tunnel.h',m.tunnel_list,True)
assert '_ppvpn_tunnel_packet_set_wakeup' in names
print(json.dumps({'header_export_contract':True,'internal':True,'export_count':len(names),'wakeup_export_present':True}))
