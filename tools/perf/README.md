# Performance checks, tiers A and B

By hand on the test host (no workflow runs it since the Go core left main; see `docs/testing.md`), these scripts measure the numbers that barely depend on the machine:

- release binary sizes;
- the engine process's RSS, both idle and under a fixed load;
- allocations;
- apply and kernel-switch time.

`report.py compare` compares them with an earlier run and with the Go 0.5.21 baseline in `testdata/perf/`; the Go engine to measure against is the v0.5.21 release file.

| File | Role |
| --- | --- |
| `measure.py` | Drives an engine that serves Core API v1 on a Unix socket with the `ppvpn-core serve` command line and log format: `ppvpn-core` today, the Rust engine's lab binary later. It starts the engine, drives it over the API, and reads `/proc` and the engine's log. It runs on Linux only. The TUN instance needs root and runs in its own network namespace. |
| `report.py` | `collect` (`PERF` lines and sizes into JSON), `merge`, `check` (numbers only, since the file is public) and `compare` (thresholds). Adapted from Sail's. |
| `../../test/perf/fakenode` | The stand-in node, on loopback: Shadowsocks 2022, AnyTLS (with a certificate generated at start, trusted through `SSL_CERT_FILE`) and an echo sink. |
| `../../test/perf/loadgen` | The fixed loads through the local proxy. |
| `internal/runtime/perf_alloc_test.go` at tag v0.5.21 | The Go engine's allocation counts (the Go core is no longer on main). They compare only with the same engine. |

## Metrics

All metrics use the same profile: one Shadowsocks 2022 node and one AnyTLS node, both on loopback.

- **standard instance** (local proxy, no TUN):
  - `idle_rss_kb`;
  - `<ss|anytls>.<stream|churn>_peak_rss_kb`. *stream* is 8 connections, 50 Mbit/s in total, 30 s, echoed back. *churn* is 200 new connections a second, 4 KiB each way, 30 s.
  - `*_after_rss_kb`, read 30 s after each load. Recorded only.
  - `apply_total_ms` and `kernel_switch_ms`, from an apply that switches kernels.
- **tun instance**: `idle_rss_kb`.
- **alloc** (Go engine only):
  - `<proto>.allocations_per_mib` and `allocated_bytes_per_mib` over a 64 MiB stream;
  - `allocations_per_connection` over 500 connections.
- **size**: the six desktop release binaries of v0.5.21 (built by that tag's `release.yml`).

## Caveats

- **RSS and size are the engine's alone.** The Rust engine runs inside the hosts' processes, so the hosts' total RSS and installer size are Desktop's measurements (#214). Against Go, sizes are a trend only.
- **The fake node's certificate is trusted only on Linux.** AnyTLS trusts it through `SSL_CERT_FILE`, which Go honours on Linux but not on macOS, so the measurement runs on Linux. The Rust engine's lab binary must accept a CA file the same way, for example a flag or `SSL_CERT_FILE` with rustls-native-certs, for this script to measure it.
- **Allocations do not compare across engines.** Go's runtime and a Rust allocator count differently.
- **Out of scope for tier A:** CPU, throughput and latency depend on the runner. Tier B measures them, below.

Run it locally on a Linux host, as root for the TUN instance:

```sh
sudo -E tools/perf/measure.py --engine-bin build/ppvpn-core --fakenode "$FAKENODE" --loadgen "$LOADGEN" --rounds 3
```

## Tier B

CPU, throughput and latency (#214's performance thresholds) depend on the machine, so they are not measured in CI. They are measured by hand on a dedicated Linux host, with the same scripts: `measure.py --tier b`. The numbers go into an issue, not into the repository.

- **Pairs.** Each round measures every engine given with `--engine`, in turn (Go, Rust, Go, Rust, ...), in one run. Go 0.5.21 and the Rust engine are therefore always compared on the same machine on the same day. Hosts are rebuilt, so numbers from different runs or days are not compared.
- **Cores.** `--engine-cpus`, `--load-cpus` and `--node-cpus` pin the engine, loadgen and fakenode to their own cores (`taskset -c`), so that the load and the fake node do not take the engine's CPU. Give them disjoint sets within the cores the job was granted.
- **Parts.** Ask for the cores a part uses, not more. `--part light` (idle CPU, latency, connections, CPU at 100 Mbit/s) needs two physical cores: one for the engine, one shared by loadgen and fakenode. `--part throughput` (unpaced streams) keeps all three busy, and the fake node (sing-box serving the proxy protocol) needs the most: ask for 8 CPUs, give the engine one physical core, loadgen one, and fakenode two. With fakenode on one core it capped AnyTLS with 8 connections for both engines (#193), and the comparison said nothing. `tput<n>_load_limited` is 1 when the engine had CPU to spare (under 80% of the CPUs it was given) while loadgen or fakenode was near its limit (85% of theirs). Such a number is the load side's; report.py does not judge it, except that a capped new number that is still not lower than the baseline passes (it is a lower bound). Run both parts on the same day, and collect their logs together.
- **Where.** The first output line, `ENV {...}`, records the CPU model, cores, memory, kernel, date and the pinning, plus `--label` values: give the job's id, for example `--label hostq_job=<id>`. The report quotes it.
- **What.** The standard instance only (local proxy), against the fake node on loopback. Nothing leaves the host.
  - idle CPU and context switches over 60 s;
  - throughput with 1 and 8 connections, unpaced, 20 s each, per protocol (`tput1_mbit`, `tput8_mbit`);
  - CPU at a paced 100 Mbit/s (`cpu100_pct`, 100 = one core);
  - 64-byte round trips through the proxy against direct ones to the sink (`extra_p50_us`, `extra_p99_us`);
  - new connections until their first byte comes back (`connect_p50_us`).
- **Thresholds** (`report.py`, against the Go engine of the same run): throughput at least 95%, extra latency p50 at most Go + 2 ms and p99 at most 110%, connection p50 at most 110%, CPU at 100 Mbit/s at most 110%, idle CPU not above Go's.
- **Not here:**
  - the TUN path's latency, until the Rust engine opens its TUN;
  - real nodes and the 24 h soak (G4, G6);
  - macOS wakeups (powermetrics) and the hosts' total RSS (Desktop, G5).
- **AnyTLS** trusts the fake node's certificate through `SSL_CERT_FILE`. The Go engine honours it on Linux. The Rust engine goes through sail's system store (`rustls-native-certs`), which honours it too, because the translation does not choose another store.

```sh
B="--tier b --rounds 3 --engine go=build/ppvpn-core --engine rust=target/release/ppvpn-core-lab --fakenode $FAKENODE --loadgen $LOADGEN"
tools/perf/measure.py $B --part light --engine-cpus 2,3 --load-cpus 4,5 --node-cpus 4,5 --label job=<id> | tee light.log
tools/perf/measure.py $B --part throughput --engine-cpus 2,3 --load-cpus 4,5 --node-cpus 6,7,8,9 --label job=<id> | tee tput.log
for e in go rust; do tools/perf/report.py collect --sha "$(git rev-parse HEAD)" --engine $e light.log tput.log > $e.json; done
tools/perf/report.py compare rust.json --baseline go.json
```
