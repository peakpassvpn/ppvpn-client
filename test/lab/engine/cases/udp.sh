#!/bin/sh
# udp: what t4 4.2 leaves out — UDP under a direct rule leaves directly;
# UDP routed to a node without UDP (us, AnyTLS, capabilities.udp=false).
# The echo on .50:9999 answers "exit=<source> <data>". QUIC sniffing is not
# covered (no QUIC client in the lab image). ids udp.<n>.
ENGINE=${1:-sing}; . /lab/cases/lib.sh
start_core $ENGINE --tun=true --local-proxy=true >/dev/null
# udp <port>: one datagram to the echo, the exit address it reports.
udp() { echo ping | nc -u -w 3 198.51.100.50 "${1:-9999}" | exit_of; }

# 1: the lab profile: UDP through the selected node (jp: .11 or .12).
apply >/dev/null && api start >/dev/null; sleep 1
check udp.1 "UDP through the selected node" "$(udp)" '198\.51\.100\.1[12]'
api stop >/dev/null

# 2: a direct rule for the echo's UDP.
jq '.revision = "udp-direct" | .routing.rules = [{"id":"udp-direct","match":{"ip_cidrs":["198.51.100.50/32"],"protocols":["udp"]},"action":{"type":"direct"}}] + .routing.rules' \
  /work/profile.json > /run/udp-direct.json
PROFILE=/run/udp-direct.json apply >/dev/null && api start >/dev/null; sleep 1
check udp.2 "UDP under a direct rule leaves directly" "$(udp)" '198\.51\.100\.100'
api stop >/dev/null

# 3: UDP routed to us, whose node and ingress say udp=false. Go 0.5.21
# routes it there anyway and sing-box carries it over the AnyTLS stream:
# the capability is not enforced (the Rust engine's choice: #45).
jq '.revision = "udp-to-tcp-only" | .routing.rules = [{"id":"udp-us","match":{"ip_cidrs":["198.51.100.50/32"],"protocols":["udp"]},"action":{"type":"proxy","target":"node","node_id":"us"}}] + .routing.rules' \
  /work/profile.json > /run/udp-us.json
PROFILE=/run/udp-us.json apply >/dev/null && api start >/dev/null; sleep 1
check udp.3 "UDP routed to a node with udp=false still goes through it" "$(udp)" '198\.51\.100\.13'
stop_core; summary
