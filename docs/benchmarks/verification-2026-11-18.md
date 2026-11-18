# Benchmark verification on 2026-11-18

The matrix harness passed 18 accounting/publication regressions and real GUI
workload checks for ztty, Ghostty and Kitty. Full 1/5/10/20 performance acceptance
remains open because the development machine is running concurrent builds and
has substantial swap usage.

## Recorded measurements

The [original ten-window data](terminals-10-windows.json) is preserved byte for
byte. Its three repetitions used shared instances, Xvfb software rendering and
DSR logical completion. The recorded one-minute load at run starts was
31.02–36.57 on 24 logical CPUs.
These timings describe that busy host; they are not a controlled latency or
throughput ranking. The regenerated SVG displays this host-load context.

The new [independent-mode validation](independent-workload-smoke-2026-11-18.json)
contains 12 successful cases: two windows, four workloads and three applications.
Each case observed two distinct GUI owner processes and no owned processes
remaining after cleanup. It is explicitly classified as `smoke`. Available RAM
ranged from 2042 to 3631 MiB, swap in use from
20656 to 22046 MiB, and one-minute load from
12.53 to 16.65. Its raw timing samples are retained
for diagnosing the harness, not for README performance comparisons.

Both datasets used the same ztty executable SHA-256:
`32e2bba63d939e419cb51b81b6b0ac54fcea8062fbc521cdbf443808321de3c9`.
The independent run's harness SHA-256 matches the source landed in
[3679b56](https://github.com/LIghtJUNction/ztty/commit/3679b569f2a0cf7fb46d840718f00f1e3c7882f2).
A later rendering-only change adds host load to the historical SVG and uses portable solid colors. Both generated figures were rasterized and visually inspected for clipping and readability.

## Tests and regressions

| Check | Result |
| --- | --- |
| `python3 tests/test_benchmark_terminals.py` | 18 passed on the development machine. |
| Shared mode, 2 windows, idle/history/images/hidden, all three applications | 12 cases passed; one GUI owner per case; cleanup complete. |
| Independent mode, 2 windows, idle/history/images/hidden, all three applications | 12 cases passed after fixing Ghostty launch isolation; two GUI owners per case; cleanup complete. |
| [Kitty image transport, 1 window, all three applications](image-transport-smoke-2026-11-18.json) | 3 cases passed, each with a successful image protocol reply. |
| Full 1/5/10/20 fresh-process matrix | Not run under the current competing workload. |
| Native GPU memory, presented first frame, input-to-display latency | Unavailable in this Xvfb harness; explicit availability fields remain false. |

The [initial isolation regression](shared-workload-isolation-regression-2026-11-18.json)
preserves the complete failed collection. All 12 shared-mode cases passed.
The next Ghostty independent-mode case was rejected because two windows still
had one GUI owner. No completed comparison was published from that collection.
The fix sets independent mode in both configuration and CLI arguments and uses
a unique Ghostty application class per window. The subsequent 12 independent
cases all passed. The failed JSON keeps its original `ok: false` status.

The accounting tests reject duplicate PIDs, missing required counters,
non-finite values, missing matrix cells, and duplicate repetitions. Smoke
datasets cannot generate the README comparison. Optional missing kernel
counters remain unavailable instead of becoming zero.

## Reproduce

Build ztty in release mode and rerun functional checks with separate artifacts:

```sh
python3 tests/test_benchmark_terminals.py
python3 tools/benchmark_terminals.py --counts 2 --modes shared,independent \
  --workloads idle,history,images,hidden --repetitions 1 --purpose smoke --no-readme \
  --json artifacts/benchmark-workloads.json --svg artifacts/benchmark-workloads.svg
```

For performance acceptance, use the same machine after competing builds have
finished and memory pressure has subsided:

```sh
python3 tools/benchmark_terminals.py --matrix
python3 tools/benchmark_terminals.py --matrix --workloads idle,history,images,hidden
```

Comparison mode checks host load and available memory before every case. It
preserves existing reports if the host is busy or a measurement fails. It does
not stop unrelated processes. Success regenerates the raw JSON, ten-window
overview and scaling SVG; all selected matrix cells must be complete.
