# Performance and validation record

Report date: **2026-11-18, Asia/Taipei**. The multiple-window section records
the shared-session implementation. The core and Kitty sections retain earlier
measurements with their original conditions. Earlier validation source:
[`9333047c0867756a73d71e29524ddef3d9f21544`](https://github.com/LIghtJUNction/ztty/commit/9333047c0867756a73d71e29524ddef3d9f21544).

The current implementation has a working Linux GUI, bounded screen/history
storage, and a production Rust dependency graph that does not link Rust `std`.
The development-machine core benchmark processes a 1 KiB ASCII burst in a median
3.138 µs. This measures parsing and screen updates only; it is separate from the
PTY measurements below and from physical key-to-screen latency.

## What each measurement means

| Layer | Measurement | Included | Excluded |
| --- | --- | --- | --- |
| Terminal core | Bytes per second; amortized feed time | Parsing, screen edits, dirty-row marking | PTY, window system, fonts, rendering, scanout |
| PTY response | Sustained query round trip | Child write, PTY, logical state, reply, child read | Physical keyboard; no rendering or presentation completion fence |
| Terminal process | RSS and PSS | Process mappings and attributable shared pages | Child process and separate X server allocations |
| X graphics | XRes pixmap bytes and X server memory delta | Server-side resources and observed server growth | Complete GPU/driver accounting |
| Visible response | Physical input to changed pixels | Input device, scheduler, application, compositor, scanout | Not measured in this report |

`no_std` controls the Rust runtime boundary. It does not itself prove low memory
or low latency. The frontend uses the system C ABI, X11, Xft, font data, and a
retained pixmap; whole-application accounting must include those separate costs.

## Multiple-window memory

The shared session process reduced total ztty PSS for eight idle windows by
67.3%, from 47.021 to 15.366 MiB. This comparison includes one GUI host and all
eight waiting launchers in the shared case. The baseline used one GUI process
per window. [Paired raw results](measurements/multiwindow-shared-2026-11-18.json)
retain every process, PTY child and X server observation.

The run used the i9-12900HX development machine, Rust 1.100.0 and isolated
software Xvfb displays. Both builds requested `monospace:size=11`, a 120×32
grid and 2,000 history rows, with opaque backgrounds, no blur and no cursor
blink. Sampling followed the same static UTF-8/color output with mostly empty
history. Baseline `bbb201c` and the shared build were interleaved for three
repetitions at each window count; all 18 runs completed and cleaned up their
owned processes.

Median process memory in MiB (1,048,576 bytes). Each entry sums unique ztty
processes; PTY children and X server allocations are recorded separately.

| Windows | Baseline PSS | Shared PSS | Baseline private | Shared private |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 10.633 | 11.172 | 7.629 | 7.504 |
| 4 | 27.275 | 12.983 | 18.848 | 9.055 |
| 8 | 47.021 | 15.366 | 37.582 | 11.301 |

Between one and eight windows, average additional PSS per window fell from
5.198 to 0.599 MiB, calculated as `(PSS at 8 − PSS at 1) / 7`.
The single-window case increased by 0.539 MiB PSS. Sharing therefore adds a
small initial process cost and removes much of the repeated per-window font
and display state; the measured savings grow with the window count.

Each window still used **3,273,728 bytes of X server pixmap storage** in both
builds. Those bytes are outside the PSS table and the 67.3% calculation.
Shells and their workloads are also separate. The shared implementation keeps
each session's screen/history and each window's pixmap independent, so these
results describe reduced process duplication rather than a complete desktop
memory total.

The machine was shared with other work and used the `powersave` governor.
PSS apportions mapped pages among active processes; private-memory totals
provide an additional view of the same run. Full-history workloads, different
fonts or grids, and hardware graphics paths require their own measurements.

Reproduce the paired comparison with both executables available:

```sh
python3 tools/measure_multiwindow.py \
  --binary ./target/release/ztty --label shared --expect-shared \
  --compare-binary /path/to/baseline/ztty --compare-label baseline-bbb201c \
  --counts 1,4,8 --repetitions 3 \
  --output artifacts/multiwindow-memory.json
```

The current MCP acceptance uses the integrated executable. Transport checks
passed for stdio, HTTP initialization and session lifecycle, IPv6, chunked
input, authentication, and 16 concurrent requests behind a stalled connection.
PTY checks passed for shared broker ownership, namespaced addressing, creator
cwd/environment/argv, resize, UTF-8 output pages, exit status, transport-exit
persistence and explicit session cleanup. The reproducible suites are
[MCP transports](../tests/mcp_transport.py),
[MCP sessions](../tests/mcp_sessions.py), and
[shared-window GUI acceptance](../tests/shared_gui_smoke.py).

## Recorded core benchmark

[Development-machine raw result](measurements/core-native-2026-11-18.json).

| Setting | Value |
| --- | --- |
| CPU | Intel Core i9-12900HX, 24 logical CPUs |
| Kernel / architecture | Linux 7.3.4-arch1-1 / x86_64 |
| Compiler | Rust 1.100.0, commit `0ef1bc386` |
| Build | Release, fat LTO, one codegen unit |
| Screen | 120 columns × 32 rows |
| History case | 2,048 requested rows, filled before timing |
| Source ref | `3e2f03297c638f14d9799dcb78a915f634287813` |
| Run timestamp | 2026-11-18 01:47:40 Asia/Taipei |

### Memory and throughput

| Metric | Measured result |
| --- | ---: |
| Cell size | 28 bytes |
| Initial terminal-core live heap | 107,676 bytes / 105.15 KiB |
| ASCII, no retained history | 232.927 MiB/s |
| ASCII, full retained history | 177.049 MiB/s |
| SGR color/control workload | 125.111 MiB/s |
| Mixed Unicode workload | 97.722 MiB/s |
| Allocations during the measured steady-state throughput loops | 0 |

The initial heap counts allocation layout sizes for a fresh `Terminal`. It
excludes allocator metadata, code, stack, fonts, window resources, and the rest
of the process. History rows are allocated on demand. The 8 MiB history bound
covers cell storage, not total process memory.

### Amortized update time

| Workload | p50 | p95 | p99 | Measured allocations |
| --- | ---: | ---: | ---: | ---: |
| Cursor-home + single ASCII overwrite | 37 ns | 41 ns | 43 ns | 0 |
| 1,024 ASCII bytes | 3.138 µs | 4.119 µs | 4.495 µs | 0 |

The harness records 10,000 samples. Each sample times 16 feed calls and divides
the elapsed time by 16. These percentiles describe batch-amortized feed cost,
not isolated key events. Throughput is the median of seven runs of 4,096 feeds
after warmup. `black_box` retains the workload and result; allocation counters
cover the timed loops.

This is a recorded run on a shared development machine, without isolated cores
or fixed-clock control. A [separate Xeon environment result](measurements/core-2026-11-18.json)
is retained for reproducibility, but is not combined with these samples or used
to calculate optimization gains across machines.

```sh
mkdir -p artifacts
cargo run --locked --release -p zterm --example benchmark \
  > artifacts/core-benchmark.json
```

## Whole-terminal measurements

This section retains the earlier single-window comparison. Use the paired
multiple-window section above to assess the shared-process change.

[Four-configuration raw results](measurements/runtime-profiled-2026-11-18.json)
and [query-only diagnostic results](measurements/runtime-query-only-2026-11-18.json).
The 12 update/query runs and six query-only runs all passed. Each used a fresh
Xvfb server; no owned terminal or child processes remained after cleanup.

The machine is the i9-12900HX described above. Both applications request the
same 120×32 character grid, `monospace` at 11 points, a 2,000-row history limit,
and the same static UTF-8/color page. `LIBGL_ALWAYS_SOFTWARE=1` is set. Memory is
sampled on the main screen before the query workload, with history mostly empty.
These results do not describe full-history or alternate-screen memory.

Each configuration is measured three times. Runs are interleaved, rotating the
starting configuration each repetition. Each run records starting load; the
one-minute load ranged from 9.73 to 14.82 during the update/query comparison.
The CPU governor was `powersave`; cores were not isolated and frequency was not
fixed. All per-run observations are retained in the JSON.

| Configuration | Explicit timing overrides |
| --- | --- |
| ztty 0.1.0 | None |
| Kitty 0.50.0, default | Installed defaults with `-c NONE` |
| Kitty 0.50.0, input-zero | `input_delay=0` |
| Kitty 0.50.0, low-latency | `input_delay=0`, `repaint_delay=2`, `sync_to_monitor=no` |

The explicit low-latency settings follow the relevant X11 timing options in
[Kitty's performance guide](https://sw.kovidgoyal.net/kitty/performance/).
The tested settings apply only to each launched process; the user's configuration
files are not modified.

### Process memory

Values below are medians of three measurements, in MiB (1,048,576 bytes).
RSS and PSS are two descriptions of process memory; do not add them together.
PSS apportions shared mappings and can vary with other running processes.

| Configuration | RSS | PSS |
| --- | ---: | ---: |
| ztty | 18.195 | 8.653 |
| Kitty, default | 155.168 | 124.787 |
| Kitty, input-zero | 155.340 | 125.315 |
| Kitty, low-latency | 155.504 | 114.102 |

This is a software-X11 comparison of different frontends and feature sets. It
is not a measurement of Kitty with its normal hardware GPU path. The same
requested font and grid also do not establish identical pixel workloads:

| Application | Actual window pixels | Font files observed in the process |
| --- | --- | --- |
| ztty | 1104×728 | ObelMono variable, NotoSansCJK Regular |
| Kitty | 1081×609 | ObelMono variable, ObelMono Italic variable, NotoSansCJK Regular, SymbolsNerdFontMono Regular |

ztty's server-side XRes pixmap allocation was **3,214,848 bytes / 3.066 MiB** in
every run. It is outside the terminal process RSS/PSS. Kitty reported zero XRes
pixmap bytes; this counter does not cover all GL resources and therefore does
not imply zero graphics memory. X server memory deltas and pixel-surface
estimates are separate fields in the raw report. They overlap with other
accounts and are not added into a claimed total.

Idle sampling found zero CPU ticks over each approximately 500 ms interval,
with a 100 Hz `/proc` accounting clock. This is a resolution-limited observation,
not a guarantee of exactly zero CPU consumption. ztty used one thread. Its
release entry executable is 155,944 bytes; that excludes system libraries and
font resources and is not compared with another application's launcher size.

### Sustained character update plus query

Each request sends cursor-home, writes the same `q` cell, and requests cursor
position. The child waits for the expected reply before issuing the next request.
Each run discards 100 warmups and records 1,000 samples. The table shows the
**median of the three per-run percentiles**, in microseconds; it is not a pooled
3,000-sample percentile or an isolated keyboard event distribution.

| Configuration | p50, µs | p95, µs | p99, µs |
| --- | ---: | ---: | ---: |
| ztty | 16.705 | 29.913 | 79.184 |
| Kitty, default | 6600.024 | 11425.976 | 13978.167 |
| Kitty, input-zero | 6968.149 | 11716.759 | 13868.448 |
| Kitty, low-latency | 6489.879 | 9927.157 | 12173.915 |

### Query-only diagnostic

This separate workload establishes the cursor position before timing, then sends
only DSR queries. It uses the same warmup/sample counts and three repetitions.

| Configuration | p50, µs | p95, µs | p99, µs |
| --- | ---: | ---: | ---: |
| ztty | 16.934 | 32.905 | 89.426 |
| Kitty, input-zero | 47.320 | 98.040 | 168.922 |

The large change for Kitty when per-request writes are removed shows that this
measurement is sensitive to the rendering workload and main-loop scheduling.
The millisecond results cannot be attributed solely to a default input delay,
or presented as a parser-speed ratio. This is an inference from the controlled
workload change, not an instrumented attribution of individual delays.

**DSR reports logical terminal state; it does not wait for rendering submission
or display presentation.** Earlier rendering can still contend with later
requests. The Python child, PTY directions, scheduler, and terminal processing
all contribute. No physical key-to-screen result or overall Kitty/Ghostty
superiority follows from these numbers. Ghostty was not measured.

Reproduce both workloads:

```sh
python3 tools/measure_runtime.py \
  --output artifacts/runtime-metrics-profiled.json
python3 tools/measure_runtime.py --query-only \
  --apps ztty kitty --kitty-profiles input-zero \
  --output artifacts/runtime-query-only.json
```

## Functional validation

The following table records earlier release validation at the source linked
at the top of this report. The shared-session and MCP checks are described
in the multiple-window section above.

| Check | Result |
| --- | --- |
| Library unit tests | 40 passed: 10 zinput, 30 zterm |
| README doctests | 3 passed |
| Clippy, all targets, warnings denied | Passed |
| Release executable build | Passed |
| `zpty` capture example | Real PTY read/write, resize, controlling TTY, child exit, and unrelated-child ownership passed |
| Rust dependency/source audit | Every production crate is `no_std`; no `std` feature enabled |
| Release symbol audit | No Rust `std::` symbols found |
| GUI acceptance | 11 passed, 0 failed under Xvfb/X11 |
| Portable-core bare-metal target | Not run successfully; target library unavailable |

All three published libraries, `zinput`, `zpty`, and `zterm`, passed individual
and combined consumers outside this workspace. Registry versions were pinned
to 0.1.0 with no local path overrides. `no_std` library checks, terminal state and
input behavior, and real PTY I/O passed. Package checksums and embedded source
commits were read back. See [the consumer report](measurements/registry-consumer-all-2026-11-18.json)
and [publication record](releasing.md).

The GUI acceptance covers legacy input, Kitty keyboard flags and repeat/release,
resize, ASCII/CJK selection, sanitized bracketed paste, lossless large paste
under backpressure, SGR mouse, gesture ownership, explicit OSC 8 opening, and
synchronized output across writes and Expose events. The large-paste fixture
delivered 139,276 bytes, including 12 delimiter bytes.

The original [GUI result](measurements/gui-2026-11-18.json) and
[12-check PTY protocol result](measurements/pty-protocols-2026-11-18.json) are
preserved. Detailed implemented and tested boundaries are in
[protocols.md](protocols.md).

![Actual ztty output showing UTF-8, ANSI colors, RGB colors, and an OSC 8 link](assets/ztty-preview.png)

## Remaining acceptance for the project goal

The paired shared-process results cover idle windows after static output,
with mostly empty history. The next acceptance work is real XWayland and input-method sessions,
full-history and alternate-screen memory, sustained TUI and Unicode workloads,
and comparison with Ghostty. Native GPU and physical input-to-visible-pixel
measurements need their own harness and comparable desktop conditions. These
are separate from the published-library and software-X11 results above.
