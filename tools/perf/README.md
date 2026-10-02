# Performance checks, tier A

`.github/workflows/perf.yml` measures, at each main push, the numbers that barely depend on the machine:

- release binary sizes;
- the engine process's RSS, both idle and under a fixed load;
- allocations;
- apply and kernel-switch time.

It compares them with the last main run and with the Go 0.5.21 baseline in `testdata/perf/`. A crossed threshold turns the workflow red. Nothing is blocked, because the workflow is not a required check.

| File | Role |
| --- | --- |
| `measure.py` | Drives an engine that serves Core API v1 on a Unix socket with the `ppvpn-core serve` command line and log format: `ppvpn-core` today, the Rust engine's lab binary later. It starts the engine, drives it over the API, and reads `/proc` and the engine's log. It runs on Linux only. The TUN instance needs root and runs in its own network namespace. |
| `report.py` | `collect` (`PERF` lines and sizes into JSON), `merge`, `check` (numbers only, since the file is public) and `compare` (thresholds). Adapted from Sail's. |
| `../../test/perf/fakenode` | The stand-in node, on loopback: Shadowsocks 2022, AnyTLS (with a certificate generated at start, trusted through `SSL_CERT_FILE`) and an echo sink. |
| `../../test/perf/loadgen` | The fixed loads through the local proxy. |
| `../../internal/runtime/perf_alloc_test.go` | The Go engine's allocation counts. They compare only with the same engine. |

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
- **size**: the six desktop release binaries, built as `release.yml` builds them.

## Caveats

- **RSS and size are the engine's alone.** The Rust engine runs inside the hosts' processes, so the hosts' total RSS and installer size are Desktop's measurements (#45). Against Go, sizes are a trend only.
- **Allocations do not compare across engines.** Go's runtime and a Rust allocator count differently.
- **Out of scope for tier A:** CPU, throughput and latency depend on the runner. They are measured before the switch, on the lab hosts and real machines.

Run it locally on a Linux host, as root for the TUN instance:

```sh
sudo -E tools/perf/measure.py --engine-bin build/ppvpn-core --fakenode "$FAKENODE" --loadgen "$LOADGEN" --rounds 3
```
