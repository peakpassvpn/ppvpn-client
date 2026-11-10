"""Focused actual-C-preprocessor checks; no product build or Flutter setup."""
import importlib.util
from pathlib import Path
import sys
import tempfile

sys.dont_write_bytecode = True
app = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('ios_exports', app / 'scripts/check-ios-exports.py')
contract = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contract)
header = app.parents[1] / 'crates/tunnel-host/include/ppvpn_tunnel.h'
public = app / 'runner/ios/PacketTunnel/Tunnel.exports'
off = contract.tunnel_contract(header, public, False)
on = contract.tunnel_contract(header, public, True)
assert contract.INTERNAL_GETTER not in off
assert on - off == {contract.INTERNAL_GETTER}
with tempfile.TemporaryDirectory(prefix='ppvpn-export-contract-') as directory:
    path = Path(directory)
    bad_public = path / 'bad.exports'
    bad_public.write_text(public.read_text() + '\n' + contract.INTERNAL_GETTER + '\n')
    for enabled in (False, True):
        try:
            contract.tunnel_contract(header, bad_public, enabled)
        except SystemExit:
            pass
        else:
            raise AssertionError('internal getter leaked into normal whitelist')
    surprise = path / 'surprise.h'
    surprise.write_text(header.read_text() + '\nvoid ppvpn_tunnel_surprise(void);\n')
    try:
        contract.tunnel_contract(surprise, public, True)
    except SystemExit:
        pass
    else:
        raise AssertionError('unexpected symbol accepted')
print('PASS export contract: actual C off/on; polluted whitelist rejected off/on; surprise symbol rejected')
