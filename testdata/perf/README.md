# Performance baselines

`baseline-go-0.5.21.json` holds the tier A numbers of the frozen Go core (v0.5.21 behaviour). `tools/perf/report.py compare` compares a run with it (see `tools/perf/README.md`).

- **Measured:** the former `perf.yml` on GitHub `ubuntu-latest` (image in `environment`), go1.26.8 with the desktop tags, Go's own allocator with GOGC and GOMEMLIMIT at their defaults. One perf run (3 rounds) plus the calibration run (10 rounds), 13 rounds in all; each metric keeps its rounds and their median.
- **Thresholds:** set from those rounds, in `tools/perf/report.py`:

  | Metric | Spread (max − min over median) | Threshold against main | Threshold against this baseline |
  | --- | --- | --- | --- |
  | Sizes | 0 | 2% | trend only (Rust links into the host; the installer is Desktop's to measure) |
  | RSS | up to 8.5% | 10% | 20% (Desktop's 120%) |
  | Allocations | 0.1–5.3% | 3–10% | 3–10%, same engine only |
  | `*_after_rss_kb` | — | record only | record only |
  | `apply_total_ms`, `kernel_switch_ms` | 75–100% (a few ms at 1 ms resolution) | record only | record only; judged on the lab hosts and real machines |

- **Re-baselining:** only by a deliberate decision, for example a new runner image that moves every number. Rerun `ci/perf-calib`, then `report.py merge` the runs with `--env` describing them.
