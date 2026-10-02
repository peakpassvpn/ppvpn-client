# Lab cases

Assertions on core behaviour that need the lab (UDP, DNS hijack, reverse
mapping, TUN, failover): one file per group, `cases/<group>.sh`, run inside
the client by `lab.sh case <group> [engine]` (engine `sing` = Go core,
default; `rust` = ppvpn-core-lab). Same script, same expectations on every
engine; that is what makes them parity cases (docs/rust-parity.md).

Write a group as:

```sh
#!/bin/sh
# <group>: what it covers; ids <group>.<n>.
ENGINE=${1:-sing}; . /lab/cases/lib.sh
start_core $ENGINE --tun=true --local-proxy=true >/dev/null && apply >/dev/null && api start >/dev/null
check <group>.1 "udp to a node" "$(echo ping | nc -u -w 3 198.51.100.50 9999 | exit_of)" '198\.51\.100\.1[123]'
...
stop_core; summary
```

- Ids are stable (`<group>.<n>`); never renumber, retire instead.
- `want` is an extended regex matched against the whole value.
- Exit addresses: .11/.12/.13 through a node, .100 direct (see ../README.md).
- `cases/<group>.baseline.txt`: the output on Go core 0.5.21 (refresh it
  only on purpose, and say why in the commit).

Groups:

| Group | Covers (beyond t4) |
|---|---|
| `dns-hijack` | IPv6 and tunnel-address DNS hijack; dns-local vs dns-remote by rule; DoT to a server not hijacked |
| `udp` | direct-rule UDP; UDP routed to a `udp=false` node |
| `reverse-map` | the reverse mapping across a kernel switch and a node change |

Not covered: QUIC sniffing (no QUIC client in the image).
