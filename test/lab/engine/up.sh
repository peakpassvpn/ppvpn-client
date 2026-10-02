#!/bin/sh
# Starts the lab: a network, the target (web), three node servers and a
# privileged client container that idles. The lab network is TEST-NET-2
# (198.51.100.0/24, internal); node ingresses are domains the node DNS
# (.54, also the client's resolver) answers. Usage: up.sh <work-dir> <sail-dir> <core-binary>
# Names and limits: lab.env (LAB_PREFIX, LAB_CPUSET, ...).
set -eu
WORK=$1; SAIL=$2; CORE=$3; HERE=$(cd "$(dirname "$0")" && pwd); NET=198.51.100
. "$HERE/lab.env"
docker rm -f $LAB-web $LAB-a $LAB-b $LAB-c $LAB-client $LAB-dns $LAB-dns-node >/dev/null 2>&1 || true
docker network rm $LAB_NETWORK >/dev/null 2>&1 || true
docker network create --internal --subnet $NET.0/24 $LAB_NETWORK >/dev/null
mkdir -p "$WORK"
docker run --rm --name $LAB-setup $LAB_RUN_OPTS -v "$WORK":/work -v "$SAIL":/sail:ro -v "$HERE":/lab:ro $LAB_IMAGE sh /lab/setup.sh
run() { name=$1; ip=$2; shift 2; docker run -d $LAB_RUN_OPTS --name $name --network $LAB_NETWORK --ip $ip --cap-add NET_ADMIN \
  --add-host www.lab.test:$NET.50 --add-host video.lab.test:$NET.50 --add-host x.video.lab.test:$NET.50 \
  -v "$WORK":/work -v "$SAIL":/sail:ro -v "$HERE":/lab:ro "$@" >/dev/null
  # The network is internal (no way out of the lab); a default route makes
  # every address routable on-link, as on a host with an uplink.
  docker exec $name ip route add default dev eth0 2>/dev/null || true; }
# The DoT upstream (what 1.1.1.1/8.8.8.8/9.9.9.9:853 are redirected to on
# the nodes) and the resolver the nodes themselves use. They answer
# split.lab.test differently: .60 (nothing there) over DoT, .50 to a node.
cat > "$WORK/Corefile.dot" <<C
tls://.:853 {
  tls /work/dot.crt /work/dot.key
  hosts {
    $NET.50 www.lab.test video.lab.test x.video.lab.test
    $NET.60 split.lab.test
    2001:db8:1::50 ipdual.lab.test
    $NET.50 ipdual.lab.test
    2001:db8:2::50 pdual.lab.test
    $NET.50 pdual.lab.test
    fallthrough
  }
  template IN A lab.test {
    match "^r[0-9]+-[a-z]\\.lab\\.test\\.$"
    answer "{{ .Name }} 30 IN A $NET.50"
    fallthrough
  }
  template IN AAAA lab.test {
    match "^r[0-9]+-[a-z]\.lab\.test\.$"
    rcode NOERROR
    fallthrough
  }
  log
}
C
cat > "$WORK/Corefile.node" <<C
.:53 {
  hosts {
    $NET.50 www.lab.test video.lab.test x.video.lab.test split.lab.test
    198.51.100.50 b1.lab.test
    2001:db8::50 dual.lab.test
    $NET.50 dual.lab.test
    2001:db8:1::50 ipdual.lab.test
    $NET.50 ipdual.lab.test
    $NET.50 pdual.lab.test
    $NET.11 jp-a.lab.test
    $NET.12 jp-b.lab.test
    $NET.13 us.lab.test
  }
  log
}
C
run $LAB-dns $NET.53 $LAB_IMAGE coredns -conf /work/Corefile.dot
run $LAB-dns-node $NET.54 $LAB_IMAGE coredns -conf /work/Corefile.node
run $LAB-web $NET.50 $LAB_IMAGE sh -c "ip addr add 10.77.0.50/24 dev eth0; exec python3 /lab/web.py /work/web.crt /work/web.key"
NODE='for ip in 1.1.1.1 8.8.8.8 9.9.9.9; do iptables -t nat -A OUTPUT -p tcp -d $ip --dport 853 -j DNAT --to-destination NET.53:853; done; exec /sail/sail -c /work/'
NODE=$(echo "$NODE" | sed "s/NET/$NET/")
run $LAB-a $NET.11 --dns $NET.54 $LAB_IMAGE sh -c "${NODE}srv-a.json"
run $LAB-b $NET.12 --dns $NET.54 $LAB_IMAGE sh -c "${NODE}srv-b.json"
run $LAB-c $NET.13 --dns $NET.54 $LAB_IMAGE sh -c "${NODE}srv-c.json"
run $LAB-client $NET.100 --privileged -v "$CORE":/usr/local/bin/ppvpn-core:ro $LAB_IMAGE sh -c 'cp /work/ca.crt /usr/local/share/ca-certificates/lab.crt && update-ca-certificates >/dev/null 2>&1; ln -sf /sail/sail /usr/local/bin/sail; ip addr add 10.77.0.100/24 dev eth0; echo "nameserver '"$NET"'.54" > /etc/resolv.conf; sleep infinity'
sleep 1; docker ps --format '{{.Names}} {{.Status}}' | grep "^$LAB-"
