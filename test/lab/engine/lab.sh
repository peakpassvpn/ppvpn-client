#!/bin/sh
# Entry point of the engine lab (host side). Names and limits come from
# lab.env (LAB_PREFIX, LAB_CPUSET, LAB_MEMORY, LAB_APK_MIRROR, LAB_WORK);
# binaries are copied into $LAB_WORK, which every container mounts at /work.
#
#   lab.sh build                                image ($LAB_IMAGE)
#   lab.sh rules                                $LAB_WORK/rs-a.srs, rs-b.srs (needs Go)
#   lab.sh up <sail> <core> <sing-box> [<ppvpn-core-lab>]
#                                               start the lab; <sail> runs the nodes;
#                                               the Rust core for engine "rust"
#   lab.sh down                                 remove the containers and networks
#   lab.sh run-all <name> <sail>                every repro on <sail> and sing-box,
#                                               then t3/t4 on $CORE_ENGINES (default sing;
#                                               "sing rust" adds the Rust core)
#   lab.sh compare <name>=<sail> ...            b1, b2-override (each build, sing-box)
#                                               and b2-direct-ipv6 (with/without) per build
#   lab.sh b7 <name>=<sail> ...                 B7: plain DNS servers under a TUN and an
#                                               interface switch (adds a second network)
#   lab.sh case <group> [engine]                cases/<group>.sh in the client (engine:
#                                               sing = Go core, default; rust =
#                                               ppvpn-core-lab)
# Logs: $LAB_WORK/repro-logs/<name>/.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/lab.env"
mkdir -p "$LAB_WORK"
cmd=${1:?usage: lab.sh build|rules|up|down|run-all|compare|b7|case ...}; shift

# put <name> <binary>: copy a binary into the work directory as <name>.
put() { cp "$2" "$LAB_WORK/$1"; chmod +x "$LAB_WORK/$1"; }
E() { out=$1; shift; docker exec -e OUT=/work/repro-logs/$out $LAB-client timeout 300 sh "$@"; }

case $cmd in
build)
	docker build -q ${LAB_CPUSET:+--cpuset-cpus $LAB_CPUSET} ${LAB_APK_MIRROR:+--build-arg APK_MIRROR=$LAB_APK_MIRROR} \
		-t "$LAB_IMAGE" "$HERE" >/dev/null
	echo "image $LAB_IMAGE"
	;;
rules)
	(cd "$HERE/mksrs" && go run . "$LAB_WORK/rs-a.srs" www.lab.test && go run . "$LAB_WORK/rs-b.srs" video.lab.test)
	;;
up)
	sail=${1:?<sail>}; core=${2:?<core>}; singbox=${3:?<sing-box>}
	mkdir -p "$LAB_WORK/nodes"
	cp "$sail" "$LAB_WORK/nodes/sail"; chmod +x "$LAB_WORK/nodes/sail"
	put core "$core"; put sing-box "$singbox"
	# A Linux build of the same architecture; the image has gcompat for a
	# glibc one (cargo build -p ppvpn-core-lab --release on Linux).
	if [ -n "${4:-}" ]; then put ppvpn-core-lab "$4"; fi
	for f in rs-a.srs rs-b.srs; do [ -s "$LAB_WORK/$f" ] || echo "warning: $LAB_WORK/$f missing (lab.sh rules, or copy them in)" >&2; done
	docker image inspect "$LAB_IMAGE" >/dev/null 2>&1 || sh "$0" build
	sh "$HERE/up.sh" "$LAB_WORK" "$LAB_WORK/nodes" "$LAB_WORK/core"
	;;
down)
	docker rm -f $(docker ps -aq --filter "name=^$LAB-") >/dev/null 2>&1 || true
	docker network rm "$LAB_NETWORK" "$LAB_NETWORK-2" >/dev/null 2>&1 || true
	;;
run-all)
	name=${1:?<name>}; put "sail-$name" "${2:?<sail>}"
	sh "$HERE/repro/run-all.sh" "$name" "/work/sail-$name"
	;;
compare)
	[ $# -gt 0 ] || { echo "usage: lab.sh compare <name>=<sail> ..." >&2; exit 2; }
	for nb in "$@"; do put "sail-${nb%%=*}" "${nb#*=}"; done
	docker exec $LAB-web iptables -C INPUT -p udp --dport 53 -j DROP 2>/dev/null || docker exec $LAB-web iptables -I INPUT -p udp --dport 53 -j DROP
	for s in b1-reverse-mapping b2-override-timing; do
		for nb in "$@"; do n=${nb%%=*}; echo "######## $s sail $n"; E compare-$n /lab/repro/$s.sh sail /work/sail-$n 2>&1 | tail -12; done
		echo "######## $s sing-box"; E compare-sing-box /lab/repro/$s.sh sing-box /work/sing-box 2>&1 | tail -12
	done
	for nb in "$@"; do n=${nb%%=*}; for v in with without; do
		echo "######## b2-direct-ipv6 $v sail $n"; E compare-$n /lab/repro/b2-direct-ipv6.sh /work/sail-$n $v 2>&1 | tail -20
	done; done
	docker exec $LAB-web iptables -D INPUT -p udp --dport 53 -j DROP
	;;
b7)
	[ $# -gt 0 ] || { echo "usage: lab.sh b7 <name>=<sail> ..." >&2; exit 2; }
	for nb in "$@"; do put "sail-${nb%%=*}" "${nb#*=}"; done
	# A second internal network (TEST-NET-3) with its own resolver, attached
	# to the client as eth1: its .54 answers split.lab.test with .60.
	docker network rm "$LAB_NETWORK-2" >/dev/null 2>&1 || true
	docker network create --internal --subnet 203.0.113.0/24 "$LAB_NETWORK-2" >/dev/null
	printf '.:53 {\n  hosts {\n    203.0.113.60 split.lab.test\n  }\n  log\n}\n' > "$LAB_WORK/Corefile.net2"
	docker rm -f $LAB-dns2 >/dev/null 2>&1 || true
	docker run -d $LAB_RUN_OPTS --name $LAB-dns2 --network "$LAB_NETWORK-2" --ip 203.0.113.54 -v "$LAB_WORK":/work "$LAB_IMAGE" coredns -conf /work/Corefile.net2 >/dev/null
	docker network connect --ip 203.0.113.100 "$LAB_NETWORK-2" $LAB-client
	for nb in "$@"; do E b7 /lab/repro/b7-dns-bind.sh sail /work/sail-${nb%%=*}; done
	E b7 /lab/repro/b7-dns-bind.sh sing-box /work/sing-box
	docker network disconnect "$LAB_NETWORK-2" $LAB-client >/dev/null 2>&1 || true
	docker rm -f $LAB-dns2 >/dev/null 2>&1 || true
	docker network rm "$LAB_NETWORK-2" >/dev/null 2>&1 || true
	;;
case)
	group=${1:?<group>}; engine=${2:-sing}
	mkdir -p "$LAB_WORK/repro-logs/cases"
	docker exec $LAB-client timeout 600 sh /lab/cases/$group.sh $engine | tee "$LAB_WORK/repro-logs/cases/$group-$engine.txt"
	;;
*)
	echo "unknown command $cmd" >&2; exit 2
	;;
esac
