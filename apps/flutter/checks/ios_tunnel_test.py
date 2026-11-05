"""Compile the real provider lifecycle with fake NE, path and host boundaries.

No app, TUN, network settings or native engine is started. Only imports,
the utun lookup and the unrelated settings/address converters are replaced;
the provider's lifecycle code is compiled unchanged. Requires Swift on CI.
An alternate --provider allows the same assertions against an older source.
"""

import argparse
import pathlib
import subprocess
import tempfile


HERE = pathlib.Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", type=pathlib.Path,
                        default=HERE.parent / "runner/ios/PacketTunnel/PacketTunnelProvider.swift")
    args = parser.parse_args()
    source = args.provider.read_text()
    # Keep the entire production lifecycle; only the OS descriptor lookup
    # and the separate settings/address conversion types are substituted.
    provider = source[source.index("final class PacketTunnelProvider:"):
                      source.index("  /// The utun the system made")]
    provider += "  static func utunDescriptor() -> Int32? { 7 }\n}\n"
    fixture = (HERE / "ios_tunnel_fixture.swift").read_text()
    tests = (HERE / "ios_tunnel_cases.swift").read_text()
    with tempfile.TemporaryDirectory(prefix="ppvpn-ne-lifecycle-") as directory:
        root = pathlib.Path(directory)
        swift = root / "main.swift"
        swift.write_text(fixture + "\n" + provider + "\n" + tests)
        subprocess.run(["swiftc", "-swift-version", "5", str(swift),
                        "-o", str(root / "lifecycle")], check=True, timeout=120)
        subprocess.run([str(root / "lifecycle")], check=True, timeout=30)


if __name__ == "__main__":
    main()
