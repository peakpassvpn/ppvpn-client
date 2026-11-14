#!/bin/sh
# App-only dependency regression gate. No dev-dependency feature unification.
set -eu
cargo check --locked -p ppvpn-core --no-default-features --lib
lightweight_graph=$(cargo tree --locked -p ppvpn-core --no-default-features --edges normal --prefix none)
if printf '%s\n' "$lightweight_graph" | rg '^(sail|sail-netstack|btls|btls-sys|quinn|quinn-proto|quinn-btls|tun|tun-rs|netconfig-rs) v'; then
    echo "lightweight core unexpectedly links forwarding/TLS/TUN dependencies" >&2
    exit 1
fi
printf '%s\n' 'lightweight-core: no Sail, BoringSSL, QUIC or TUN dependency'
