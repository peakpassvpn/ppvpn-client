"""Execute actual public packet provider lifecycle against controlled OS boundaries.
No OS settings, real packet flow, app or native engine is started. The production
provider body and completion gate are compiled unchanged; only imports and the
unrelated address/settings converter types are replaced. SDK typecheck/build and
real isolated-device acceptance remain independent gates.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

here = Path(__file__).resolve().parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--provider', type=Path, default=here.parent / 'runner/ios/PacketTunnel/PacketTunnelProvider.swift')
args = parser.parse_args()
source = args.provider.read_text()
assert 'utunDescriptor' not in source and 'getsockopt' not in source
provider = source[source.index('final class PacketTunnelProvider:'):source.index('\nstruct TunnelError:')]
completion = source[source.index('private final class PacketSettingsCompletion:'):]
with tempfile.TemporaryDirectory(prefix='ppvpn-public-packet-tests-') as directory:
    work = Path(directory)
    swift = work / 'main.swift'
    swift.write_text((here / 'ios_packet_fixture.swift').read_text() + '\n' + provider + '\n' + completion + '\n' + (here / 'ios_packet_cases.swift').read_text())
    subprocess.run(['swiftc', '-swift-version', '5', '-module-cache-path', str(work / 'modules'), str(swift), '-o', str(work / 'provider-tests')], check=True, timeout=120)
    subprocess.run([str(work / 'provider-tests')], check=True, timeout=60)
