# Performance and validation record

Report date: **2026-11-18, Asia/Taipei**. Current production source:
[`7535aa29f1a2728a78a446ffea6784dcbc2aae92`](https://github.com/LIghtJUNction/ztty/commit/7535aa29f1a2728a78a446ffea6784dcbc2aae92).

Version 0.2 adds bounded Kitty graphics and native macOS/Windows backends while
retaining the Rust `no_std` production boundary. The measured text-only initial
core heap remains 107,676 bytes. A same-host paired core benchmark records a
2.232 µs median amortized 1 KiB feed for the graphics implementation. A separate
Linux development-machine run records an 18.36 MiB median process RSS and
16.456 µs median-of-run p50 PTY response. These measure different layers and
machines; neither establishes physical key-to-screen latency.

Current measurements are presented first. The original 0.1 measurements,
including its Kitty comparison, remain below with their historical source and
conditions. No new comparative Kitty or Ghostty performance run was performed
for 0.2, and native macOS/Windows performance has not been measured.

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

## Version 0.2 paired core measurement

[Raw paired benchmark](measurements/graphics-core-overhead-2026-11-18.json).
The baseline is the core before graphics at `bbb201c`. The graphics variant
contains the final bounded streaming receiver and repeated-action/unpadded
Base64 compatibility fixes. Both variants ran on the same local machine with
CPU 0 affinity, in three interleaved runs. This machine is separate from the
i9 development machine used for the runtime measurements below. The record
records the candidate source-file hashes and both benchmark binary hashes.
The later DA1 numeric serialization fix changes a
query reply, outside these text-feed workloads.

The table contains medians of three runs. The latency percentiles are medians
of per-run percentiles, not percentiles recomputed from a pooled sample set.

| Metric | Baseline | Graphics variant |
| --- | ---: | ---: |
| ASCII throughput | 313.713 MiB/s | 312.621 MiB/s |
| ASCII with full history | 209.947 MiB/s | 220.766 MiB/s |
| SGR workload | 172.967 MiB/s | 171.978 MiB/s |
| Mixed Unicode workload | 114.261 MiB/s | 108.145 MiB/s |
| 1 KiB feed, p50 | 2.168 µs | 2.232 µs |
| Single-cell edit, p50 | 32 ns | 36 ns |
| Initial live core heap | 107,676 B | 107,676 B |
| Measured steady-state allocations | 0 | 0 |

The graphics variant's 1 KiB p95 and p99 are 3.887 and 7.040 µs. Each latency
sample amortizes 16 feed calls; the harness records 10,000 samples per run.
The 120×32 screen uses 28-byte cells. Initial heap counts allocator layout
sizes, excluding allocator metadata, code, stacks, fonts, window buffers, and
other process state. Empty graphics scenes allocate no image storage.

In these samples ASCII throughput changed by about -0.35%, SGR by -0.57%,
mixed Unicode by -5.35%, and full-history ASCII by +5.15%. The 1 KiB p50 rose
about 2.95%. Three shared-environment runs do not establish a statistically
stable improvement or regression for small differences. Individual runs remain
in the JSON rather than being discarded. These results justify keeping the
ordinary VT byte stream batched; they do not describe image decode speed or
latency while images are arriving.

## Version 0.2 Linux process and PTY measurement

[Three-run runtime record](measurements/runtime-graphics-2026-11-18.json).
The environment is the i9-12900HX development machine, Linux 7.3.4-arch1-1,
Rust 1.100.0, and a fresh software Xvfb server per run. Each window requests
120×32 cells, `monospace` at 11 points, and 2,000 history rows. The observed
window is 1112×736 pixels with Obel Mono and Noto Sans CJK font files. History
is mostly empty and no images are retained in this text-page workload.

The measured local executable is 296,080 bytes. Its transferred binary hash was
`a5e2a7c0a30ae95ee7091ff676e934dc9a39939bf4f5d509e761bd2ac8b3a329`.
This run precedes the DA1 serialization fix and is not a performance measurement
of the later source-bound native workflow binaries. The raw measurement JSON
records its path and version, rather than a source SHA; that provenance limit
is retained here.

| Metric | Median of three runs |
| --- | ---: |
| Terminal process RSS | 18.359 MiB |
| Terminal process PSS | 13.586 MiB |
| XRes pixmap resources | 6,547,456 B / 6.244 MiB |
| PTY home-write-query p50 | 16.456 µs |
| PTY home-write-query p95 | 34.915 µs |
| PTY home-write-query p99 | 101.634 µs |

The per-run p50 values were 13.076, 23.652, and 16.456 µs; p99 ranged from
38.957 to 341.653 µs. The machine was shared, used the `powersave` governor,
and did not isolate cores or fix CPU frequency. Each run discarded 100 warmups
and measured 1,000 sequential requests. The reported percentiles are medians of
the three per-run percentiles, not a pooled 3,000-sample distribution.

RSS and PSS describe the terminal process, excluding its Python child and X
server. They are alternatives, not additive accounts. XRes measures separate
server resources attributable to the client. Its count happens to equal two
window-sized 32-bit surfaces, but this observation does not identify allocation
ownership or account for complete GPU/driver memory. The single-surface estimate
and server memory deltas are separate fields and should not be summed blindly.

The idle intervals observed zero `/proc` CPU ticks over approximately 500 ms,
with a 100 Hz accounting clock and one terminal thread. This is a measurement
below tick resolution, not a guarantee of literally zero CPU consumption.
**DSR replies report logical state and do not wait for rendering or display
presentation.** PTY, scheduler, child interpreter, and terminal processing all
contribute to these response times.

```sh
python3 tools/measure_runtime.py --apps ztty --repeats 3 \
  --ztty ./target/release/ztty --output artifacts/runtime-graphics.json
```

## Version 0.2 functional and native acceptance

| Gate | Observed result | Evidence |
| --- | --- | --- |
| Library tests | 72 passed: 23 zgraphics, 39 zterm, 10 zinput | [Local gate log](measurements/local-gates-2026-11-18.txt) |
| README doctests | 4 passed | Same local log |
| Strict workspace Clippy | All targets passed with warnings denied | Same local log |
| Linux release build | Passed | Local log source/binary identity; native artifact records |
| Rust production audit | 11 production crates use no_std without an enabled std feature; no linked Rust std symbols | [Audit](measurements/no-std-graphics-2026-11-18.txt) |
| Portable target | zinput, zgraphics, zterm pass x86_64-unknown-none checking | Local gate log |
| Existing X11 GUI | 12 checks passed | [GUI report](measurements/gui-graphics-2026-11-18.json) |
| Xvfb graphics | 7 groups passed with actual PTY and pixel readback, including real icat automatic detection | [Graphics report](measurements/graphics-xvfb-2026-11-18.json) |
| Hyprland/XWayland | Actual photo, transparent PNG, and stream detection passed | [Desktop report](measurements/graphics-hyprland-2026-11-18.json) |
| Registry packages | Four individual and one combined exact-version consumer passed | [Registry consumers](measurements/registry-native-graphics-2026-11-18.json) |
| macOS Apple Silicon and Intel | Native SDK builds and real AppKit/Darwin PTY, 8 session/protocol checks each | [ARM64](measurements/ztty-macos-aarch64.native-7535aa2.json), [Intel](measurements/ztty-macos-x86_64.native-7535aa2.json) |
| Windows x86_64 | 17 checks with actual ConPTY and Win32 pixel readback | [Windows report](measurements/ztty-windows-x86_64.native-7535aa2.json) |

The native reports belong to source `7535aa2` and
[formal artifact run 44165748395](https://github.com/LIghtJUNction/ztty/actions/runs/44165748395).
Both Linux architectures also built and passed native CLI startup in this run.
Windows ARM64 has a verified [cross-linked bundle](measurements/windows-arm64-cross-2026-11-18.json),
but was not executed on Windows ARM64. Both macOS reports explicitly record that
pixel readback and IME/keyboard checks were not performed. These gaps are not
filled by successful cross compilation or protocol replies.

The bare-metal check succeeds with pinned Rust 1.100.0 and reports a
future-compatibility warning in zune-png 0.5.2's x86 SIMD target-feature functions
for that soft-float target. This dependency limitation should be revisited when
updating Rust or supporting that target as an executable platform; it did not
fail the portable library check or the native platform builds.

Graphics acceptance includes alpha 0/64/128/255, crop, z ordering, replacement,
scroll, resize, alternate screens, and deletion. After earlier image tests
warmed the process, 80 interrupted uploads produced 0 bytes of observed RSS
growth between sampled endpoints, and subsequent keyboard/text/image input
worked. This does not measure peak decode memory. Default image quotas are per
scene, and temporary decoding and renderer resources have separate costs; see
[architecture memory accounting](architecture.md#memory-accounting).

Remaining performance acceptance includes full-history and image-heavy process
memory, sustained mixed TUI workloads, native macOS/Windows timings, hardware GPU
workloads, physical input-to-visible-pixel latency, and a comparable Ghostty run.
Linux's damaged-row renderer and the native frontends' full-frame paths must be
measured separately. Functional desktop image acceptance is now complete for
the stated Linux tests; a performance claim of overall Kitty/Ghostty superiority
has not been established.

## Historical 0.1 core benchmark

Use the recorded historical source revision when reproducing the 0.1 commands
below. Running them from the current checkout measures the current implementation.

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

## Historical 0.1 whole-terminal measurements

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

## Historical 0.1 validation

The following results belong to the original Linux implementation at
`9333047c0867756a73d71e29524ddef3d9f21544`, with the benchmark recorded at
`3e2f03297c638f14d9799dcb78a915f634287813`. They are retained as historical
observations; the 0.2 results and limits are recorded above.

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
