# Roadmap idle matrix verification — 2026-11-18

All 24 idle scenarios completed successfully, with no owned processes remaining
after cleanup. The single-window RSS target below 10 MiB was **not met**:
the application process set used 22.06 MiB; separate mapping inspection found
18.02 MiB in the GUI broker and 4.04 MiB in the launcher.

This is one fresh-process run per scenario on a busy Linux development host.
It does not replace the README baseline or establish stable performance rankings.
Filenames use the roadmap delivery date; raw collection timestamps are
2026-11-17 UTC.

## Evidence and source identity

- [Raw matrix](roadmap-idle-matrix-smoke-2026-11-18.json)
- [PSS scaling figure](roadmap-idle-matrix-smoke-2026-11-18.svg)
- [Binary identity and 52-source-file manifest](roadmap-idle-binary-2026-11-18.json)
- [Source reconstruction patch](roadmap-benchmark-source-2026-11-18.patch)
- [Reconstruction verification](roadmap-benchmark-source-verification-2026-11-18.json)
- [Three smaps snapshots](roadmap-idle-smaps-2026-11-18.json)
- [Reproducible smaps sampler](roadmap_idle_smaps.py)

Measured binary: private development-machine copy
`artifacts/roadmap-benchmark-ztty`, 631608 bytes, SHA256
`89db6aa306ce5d2291829b94f8814e8b222a796991c9eea5eadb402881867b13`.
It was copied once after the release build, before subsequent development
rebuilt the normal target.

It was built from a dirty source snapshot, **not directly from commit
52bdda721010ef0e9f2099cf8a5a7da3c4b2f1ad**. Comparing all 52 recorded source hashes
against that later commit found one difference: configuration-error text ordering
in `src/appearance.rs`. Both `git apply --check` and actual patch application to
files extracted from that commit succeeded in a temporary directory. All 52
reconstructed hashes match the manifest. Development sources were untouched.
This establishes source reconstruction, not reproducible binary identity under
a different compiler or linker.

The matrix retains its original collector hash, timestamp, binaries, settings,
and raw runs. A separate `derived` section records the updated renderer hash and
unchanged raw-run hash. Paired application plus full Xvfb PSS and sampled Swap
come from existing counters. Missing counters remain null.

## Same-host observations

Values are MiB, the median of three nearby memory snapshots within one run per
cell. RSS counts shared pages in each process; PSS apportions shared resident
pages. Swap remains separate and is not added to PSS as unique physical memory.
Application totals exclude Xvfb, PTY workload children, and private D-Bus.
Full dedicated Xvfb PSS is available in each derived row as
`median_app_plus_xserver_pss_bytes`.

| Windows | Terminal | Shared RSS | Shared PSS | Shared Swap | Independent RSS | Independent PSS | Independent Swap |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | ztty | 22.06 | 14.52 | 0.00 | 22.04 | 14.34 | 0.00 |
| 1 | ghostty | 338.69 | 319.75 | 0.00 | 332.58 | 313.23 | 0.00 |
| 1 | kitty | 210.46 | 183.30 | 0.00 | 208.71 | 181.67 | 0.00 |
| 5 | ztty | 39.04 | 16.73 | 0.00 | 110.02 | 35.53 | 0.00 |
| 5 | ghostty | 560.71 | 540.99 | 0.00 | 1470.26 | 873.18 | 0.00 |
| 5 | kitty | 299.15 | 266.01 | 0.00 | 949.56 | 500.72 | 0.00 |
| 10 | ztty | 60.21 | 19.55 | 0.00 | 219.41 | 61.61 | 0.00 |
| 10 | ghostty | 838.07 | 818.03 | 1.95 | 2842.62 | 1522.26 | 19.41 |
| 10 | kitty | 403.26 | 363.00 | 0.00 | 1878.31 | 887.74 | 19.04 |
| 20 | ztty | 102.57 | 24.85 | 0.00 | 439.88 | 112.66 | 0.29 |
| 20 | ghostty | 1366.21 | 1344.56 | 1.97 | 4203.38 | 1422.06 | 1385.24 |
| 20 | kitty | 599.67 | 544.71 | 2.00 | 3723.72 | 1656.46 | 38.05 |

At 20 independent Ghostty windows, approximately 1385.24 MiB was swapped out.
Its resident PSS fell below its 10-window value. That is evidence of paging,
not improved memory scaling. The figure now displays maximum sampled
application Swap and explicitly warns about resident-memory interpretation.

At case start, load1 ranged from 13.07 to 26.18 on 24 logical CPUs,
MemAvailable from 2002.25 to 3681.88 MiB, and host swap used from
21489.21 to 23663.15 MiB. The CPU governor was powersave. Other users' builds
were left running. The idle matrix was limited to 24 cases to avoid adding
the full 96-case workload pressure under these conditions.

The configuration used 120×32 cells, monospace 11 pt at 96 DPI, opaque windows,
disabled blur/blinking, a fixed multilingual color frame, and isolated Xvfb
with software rendering. Binary versions: ztty 0.2.0, Ghostty 1.4.0-arch1,
Kitty 0.50.0; exact executable hashes and distribution provenance are in JSON.
Shared mode uses one ztty broker, Kitty single-instance grouping, or Ghostty
GTK single-instance grouping. Independent mode verifies distinct GUI owners
and uses a separate broker socket for each ztty window.

## Why RSS exceeds 10 MiB

The separate sampler used the same immutable binary and warmup frame.
It identified GUI ownership through `_NET_WM_PID`, exact executable paths,
private run markers, and process start ticks. Xvfb and the PTY child were
excluded and identified separately. All three snapshots measured 18.02 MiB
broker RSS and 4.04 MiB launcher RSS, with zero Swap. Owned-process cleanup
completed with successful close and launcher exit.

| Broker mapping category | RSS (MiB) |
| --- | ---: |
| Shared libraries | 6.637 |
| Fontconfig cache mappings | 3.531 |
| Heap | 3.277 |
| Font files | 2.016 |
| Anonymous mappings | 1.301 |
| ztty executable | 0.523 |
| Dynamic-loader cache | 0.305 |
| Other files | 0.234 |
| Stack | 0.180 |
| Kernel special mappings | 0.016 |

Libraries, font caches, and fonts account for about 12.18 MiB before heap and
other mappings. The CJK warmup loads fallback-font data. The next investigation
should examine fontconfig mapping lifetime, loaded font/library pages, and the
resident launcher. Smaps is not an allocation profiler: this evidence does not
identify heap allocation owners or establish that individual mappings can be
removed safely. Even excluding the launcher, the current broker exceeds 10 MiB.

## Reproduction and checks

Run from the repository root using an immutable executable and fresh output paths:

```sh
python3 tools/benchmark_terminals.py \
  --ztty ./artifacts/roadmap-benchmark-ztty \
  --matrix --workloads idle --repetitions 1 --purpose smoke --no-readme \
  --json docs/benchmarks/roadmap-idle-matrix-smoke-2026-11-18.json \
  --svg docs/benchmarks/roadmap-idle-matrix-smoke-2026-11-18.svg
python3 docs/benchmarks/roadmap_idle_smaps.py --help
python3 tests/test_benchmark_terminals.py
```

The merged benchmark suite passed 23 tests, covering paired PSS/Swap, signed
X server deltas, unavailable and invalid counters, complete matrices, smoke
publication guards, preservation of raw data, and graphics-transfer framing.
Both generated SVGs were rasterized and visually inspected.

The README baseline JSON stayed byte-identical during SVG rendering, SHA256
`d41310c9d47955689cb559965a974cdebc9f6d720869c32b4efad82091f6967a`.
Only that SVG was regenerated; its observations describe the earlier baseline
binary, not the new roadmap binary.

## Remaining measurements

Native GPU/VRAM, startup-to-first-frame, input-to-display latency, full history,
image load, and hidden-session measurements remain outstanding. PTY DSR replies
measure logical completion, not rendered frames; timings on this busy, swapping
host are diagnostic. The harness's hidden workload unmaps OS windows and does
not measure inactive tabs. Quiet-host repeated trials, real GPU instrumentation,
native macOS/Windows tests, and the other workloads remain required.
