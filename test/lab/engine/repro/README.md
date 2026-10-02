# Engine gap repros

One script per gap found while running ppvpn-core's behaviour on Sail
(first on the prototype, now the reference for ppvpn-engine). Each script
prints what it saw and writes the engine's debug log next to it. Wherever
the same JSON is valid for both, it runs on **Sail and on sing-box 1.13.12**
(the version Go core embeds). Lab keys are generated per lab and redacted
(`<ss-server-key>`, `<uuid>`, ...) in every config copied into the logs. CLI
started as `sail -c <file>`; nothing uses the FFI.

Run them through `../lab.sh` (see `../README.md`): `lab.sh run-all`,
`lab.sh compare`, `lab.sh b7`; one by hand inside the client:
`docker exec $LAB-client sh /lab/repro/<script>.sh <engine> /work/<binary>`.
Logs land in `$LAB_WORK/repro-logs/<name>/`.

Hosts: lab-client 198.51.100.100 (privileged, runs the engine), lab-web .50
(echoes `exit=<source> host=<Host>`, `/generate_204`, UDP echo on 9999),
lab-a .11 Sail VLESS+REALITY server, lab-b .12 Sail SS2022 server, lab-c .13
Sail AnyTLS server, lab-dns .53 DoT (CoreDNS), lab-dns-node .54 plain DNS.
**Exit .12 means the connection went through the node; .100 means direct.**

## Index

| Item | Script | Engines | What it shows |
| --- | --- | --- | --- |
| B1 reverse mapping | `b1-reverse-mapping.sh` | both | Hijacked answer b1.lab.test → .50; UDP to .50:9999 then matches the domain rule on sing-box (→ node, log `match[2] domain_suffix=b1.lab.test => route(node)`), on Sail goes direct and no `dns reverse mapped` line appears. Sail only learns from port-53 UDP answers that pass through an outbound (`dispatcher.rs` `SniffingDatagram`), never from hijack-dns. (A TCP HTTP/1.0 request without Host goes direct on both: sing-box's HTTP sniffer replaces the mapped name with nothing.) |
| B2 override timing | `b2-override-timing.sh` | both | sniff + `ip_cidr 198.51.100.50/32 → direct` + final node; a request to 198.51.100.50 with Host split.lab.test: sing-box (sniff, no override) → direct; Sail (sniff with `override_destination`) rewrites at the sniff rule, the ip_cidr rule no longer matches → node. In ppvpn-core the ip_cidr rule is the private ranges, so LAN traffic with a name leaks to the node. |
| B3 ordered DNS fallback | `b3-dns-fallback.sh` | Sail | `race` over a server that answers (.54) and one that drops (.50): the capture shows every query sent to both at once. Wanted: a server type that asks the first, then the next only after a failure/per-try timeout, SERVFAIL within an overall budget (ppvpn-core: 3 s per try, 8 s total, primary 1.1.1.1, then 8.8.8.8, 9.9.9.9). |
| B4 fallback group | `b4-fallback-group.sh` | Sail (+ core t3 on both) | Member a dropped: the next new connections each wait the full `timeout` (5 s) before b; a is still `alive:true`, `history:[]` in the Clash API; switches back on the first passed test, no hysteresis; no switch event. `logs-<build>-t3-*.txt` are the same scenario through ppvpn-core on both engines, incl. the core-managed variant. |
| B2 direct (#48) | `b2-direct-ipv6.sh` | Sail (`without` / `with`) | Host without an IPv6 path, TUN routing IPv6: a direct connection to dual.lab.test's AAAA (2001:db8::50) must go out over IPv4 to the name with `{"inbound":["tun"],"ip_cidr":["2000::/3"],"action":"route-options","override_destination":"proxy_and_direct"}` and direct `domain_resolver` `ipv4_only`. Cases: HTTP Host, TLS SNI, no name (reverse map), UDP (reverse map), and UDP again after an in-place reload (map kept). `without` on v0.15.0: TCP reset at once (rc=56/35), UDP no reply; `with` is refused until B2 lands. |
| DNS cookie replay | `dns-cookie-replay.sh` | both | A hijacked query answered from Sail's cache carries the cookie of the query that filled the cache: `dig +cookie` shows `(bad)` from the 2nd query on; c-ares (curl `--dns-servers`) times out on the cached name. sing-box echoes each client's cookie. Found 2026-10-02, reported to Sail. |
| 7 dial through an outbound | — (prototype code) | — | No public API to dial via a named outbound. The prototype adds a hidden inbound: `internal/sailengine/translate.go` (`FlowListen`), one user per outbound tag and `{"inbound":"core-flow","auth_user":"<tag>","outbound":"<tag>"}` rules. Uses: mobile per-flow streams (TCP+UDP) and rule-set downloads through `direct` while the TUN is up. Design agreed with Sail (2026-10-02). |
| 9 inbound without port | `item09-inbound-no-port.sh` | Sail | `POST /api/v1/runtime/inbounds {"type":"mixed","tag":"no-port"}` → 200, `added inbound [no-port]`, nothing listens. |
| 10 Host rewrite | `item10-host-rewrite.sh` | both | Through the mixed inbound's HTTP proxy the target receives `Host: 198.51.100.50:80` (Sail) vs `198.51.100.50` (sing-box, and curl without a proxy). |
| 11 rule without conditions | — | both | `{"route":{"rules":[{"action":"reject"}]}}`: `sail -T` exits 1, "the rule has no conditions"; sing-box accepts (matches everything). |
| 12 error reporting | `item16-reload-untagged.sh`, `item15-ruleset.sh` | Sail | Reload of an invalid file: 202, empty body, no log line with the reason. Truncated .srs: `route.rule_set[0]: [rs]` with no cause (sing-box: `parse rule-set[0]: read rule[0]: unexpected EOF`). `-T` prints no warnings (e.g. `tun.stack` is only warned at start). |
| 13 ip rules after a crash | core-level (`t*`); see below | Sail | `kill -9` / `kill -ABRT` of sail with a TUN: utun is gone, the 5 `ip rule` entries (9000–9003) stay until the next sail starts and stops. |
| 15 rule-set file watch, realm | `item15-ruleset.sh` | both | Replacing the .srs is picked up by sing-box within 3 s, by Sail only after a reload. A corrupt file: sing-box logs `ERROR router: reload rule-set rs: read rule[0]: unexpected EOF` and keeps the old set; Sail keeps the old set on reload (202, no reason). 407 realm: `sail` vs `sing-box`. |
| 16 reload "hang" | `item16-reload-untagged.sh` | Sail | **Retracted**: the hang was the test script (`wait` on the core started with `&`). Reload of an untagged unknown outbound answers 202 in 0 ms; API and traffic keep working, on both builds. |
| macOS | — | Sail | Official macOS universal CLI: `-T` refuses `tun.iproute2_table_index` ("Linux only"), which ppvpn-core renders for every platform and sing-box ignores off Linux (core will drop it; a warning would match sing-box). Binary is ad-hoc signed only. |

Item 13 by hand, in lab-client with any TUN config running under sail:
`kill -9 $(pgrep -x sail); ip rule; ip -br link | grep utun`.

Core-level logs (`logs-<build>-t3-<engine>.txt`, `logs-<build>-t4-<engine>.txt`)
run the whole ppvpn-core (`serve`) on each engine: t3 is ingress failover and
pin, t4 is TUN + DNS (hijack, sniff handoff, reverse mapping, pooled/half-open
DoT, all upstreams failing, one upstream failing).
