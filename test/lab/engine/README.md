# Engine lab

A Docker lab in which the same scripts drive core's behaviour on two engines:
the Go core with sing-box (`ppvpn-core serve`) and, for Sail, the standalone
`sail` binary (repros) or an engine with a Sail kernel behind the same
control surface (core-level tests; for ppvpn-engine, see ppvpn-core#45:
`ppvpn-engine-lab`). It is gate G2 of #45 and the place where Sail gaps are
reproduced and regressed. Moved here from the prototype branch
`proto/sail-engine` (archived; its Go engine code is not part of main).

Everything is local to the lab: an internal network on TEST-NET-2
(198.51.100.0/24; TEST-NET-3 for the B7 second network), names under
`.lab.test`, keys and certificates generated at `up`.

| Host | Address | Role |
|---|---|---|
| `$LAB-client` | .100 | privileged; runs the engine under test (core, sail, sing-box) |
| `$LAB-web` | .50 | target: echoes `exit=<source> host=<Host>`, `/generate_204`, UDP echo on 9999 |
| `$LAB-a`, `-b`, `-c` | .11, .12, .13 | nodes (Sail servers): VLESS+REALITY, SS2022, AnyTLS |
| `$LAB-dns` | .53 | DoT upstream (what 1.1.1.1/8.8.8.8/9.9.9.9:853 lead to) |
| `$LAB-dns-node` | .54 | plain DNS of the nodes and the client; answers the node ingress domains |

**Exit .11/.12/.13 means the connection went through that node; .100 means
direct.** Node ingresses are given by domain only (`jp-a.lab.test`, ...), as
the backend sends them without a pinned entry IP.

## Run

Needs Docker, a privileged container (TUN, iptables), and Linux x86_64 or
arm64 binaries: a core (`ppvpn-core`, built with `make build-linux-artifact`
or the release file), a Sail for the nodes and as the subject, and sing-box
1.13.12 (`go build -tags with_gvisor,with_utls github.com/sagernet/sing-box/cmd/sing-box@v1.13.12`).

```sh
cd test/lab/engine
export LAB_WORK=$PWD/work
./lab.sh rules                                   # rs-a.srs, rs-b.srs (needs Go)
./lab.sh up <sail> <ppvpn-core> <sing-box>       # build the image if missing, start the lab
./lab.sh run-all 0.16.0 <sail>                   # every repro + t3/t4 on the Go core
./lab.sh compare 0.16.0=<sail> v0.15.0=<sail>    # b1, b2 against sing-box; b2-direct-ipv6
./lab.sh b7 0.16.0=<sail>                        # B7: DNS servers under a TUN, interface switch
./lab.sh down
```

Logs: `$LAB_WORK/repro-logs/<name>/`.

**Shared hosts.** Every name and limit is a parameter (`lab.env`): give the
lab its own prefix and the CPUs a job queue granted, and an Alpine mirror if
the default CDN is unreachable:

```sh
LAB_PREFIX=core-lab LAB_CPUSET="$GRANTED_CPUS" LAB_MEMORY=2g \
LAB_APK_MIRROR=https://mirrors.example.org ./lab.sh up ...
```

Copy results off the host after each run; nothing outside `$LAB_WORK` is
written.

**Engines of the core-level tests** (`t3` ingress failover and pin, `t4` TUN
and DNS, `t56` local proxy, `t9`): `CORE_ENGINES` (default `sing`, the Go
core). A Sail engine (`sail`) needs a core that runs one behind the same
control endpoints (`core.sh` passes `PPVPN_ENGINE`/`PPVPN_SAIL_*` through):
the prototype did; ppvpn-engine will through `ppvpn-engine-lab`.

## Files

- `lab.sh`, `lab.env`: entry point and parameters; `up.sh`, `setup.sh`:
  network, keys, certificates, node configs, profile; `Dockerfile`: image
  (Alpine 3.20 pinned by digest); `web.py`: the target.
- `core.sh` (in the client): start the core, call its API; `t3*.sh`, `t4-host.sh`,
  `t56.sh`, `t9.sh`: core-level checks.
- `repro/`: one script per gap with its index (`repro/README.md`). Run
  logs are never committed (tools/sensitive-check.sh); results are kept as
  the table below.
- `mksrs/`: writes sing-box binary rule sets (`.srs`).

## Results so far

Sail 0.15.0 (867e441/41052b8, 2026-10-02) and 0.16.0 (dfe83b8, 2026-10-03),
against sing-box 1.13.12 (run logs stay outside the repository):

| Item | 0.15.0 | 0.16.0 |
|---|---|---|
| B1 reverse mapping (hijacked answers; TCP without Host, UDP to the address) | not mapped: direct | to the node by name (sing-box clears the name of Host-less HTTP: direct) |
| B2 override at dial time (private rule after sniff) | sniff rewrote: to the node | rule holds: direct (= sing-box) |
| B2 direct IPv6 hand-off (`proxy_and_direct`, 5 cases) | option refused | all 5 out over IPv4 direct |
| B3 ordered DNS fallback | only `race` (asks all) | `sequential`: in order, 3 s per try, prefer_for, SERVFAIL at the 8 s budget |
| B4 fallback group | every new connection waits the full timeout; alive:true, no history | one failed dial marks the member; history visible; INFO switch line; ~15 s before switching back |
| B5 runtime API | unauthenticated | secret required, loopback only, 401, api.sock 0600 |
| B6 DNS cookie (dig, c-ares) | cached answers replay a stale cookie: timeouts | each client's cookie echoed (= sing-box) |
| B7 DNS servers' sockets under a TUN | loop through the TUN (also 0.15) | loop through the TUN; `bind_interface` avoids it (fix pending, #45) |
| 9 inbound without a port | 200, listens nowhere | 400 with the reason |
| 10 HTTP proxy Host | rewritten with `:80` | kept (= sing-box) |
| 12a reload errors | 202, no reason | 200 / 400 with the reason, config kept |
| 15a rule-set file | needs a reload | followed within 3 s; corrupt file: ERROR, old rules kept |
| t3 ingress failover (sail engine) | 1 fail, 2 slow | 1 fail, 1 slow (= sing-box) |
