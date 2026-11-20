# Linux bounded repaint: A/B and acceptance, 2026-11-18

The Linux frontend now batches ordinary continuous-output redraws behind a fixed
8 ms scheduling budget while continuing to parse PTY bytes and answer protocol
requests. In the six-run, one-window comparison below, median logical PTY
throughput increased from **18.88 to 133.01 MiB/s**. Every run displayed a fresh
final marker before receiving any further PTY request, and all six final window
images were byte-identical. This result applies to the recorded ASCII/CUP/SGR
workload on private Xvfb. It does not establish physical display latency or a
general speedup over Kitty or Ghostty.

[Unmodified A/B data](repaint-ab-2026-11-18.json) ·
[Exact pinned collector](repaint-ab-2026-11-18-collector.py) ·
[New GUI results](repaint-gui-2026-11-18.json) ·
[Four-suite native gate](repaint-linux-native-2026-11-18.json) ·
[Earlier X11 diagnostic](repaint-x11-diagnostic-2026-11-18.json)

## What changed

Repeated full-window submission was a measurable cost in the previous frontend.
The change separates terminal parsing from the decision to redraw; it does not
increase the per-session PTY read allowance or postpone DSR replies. The existing
four-read allowance and 16 KiB read buffer remain intact.

The scheduling rules are implemented in [`src/repaint.rs`](../../src/repaint.rs)
and connected to [`src/session.rs`](../../src/session.rs):

| Event | Scheduling rule |
| --- | --- |
| Ordinary output changes the visible state | Paint immediately if the previous completed paint was at least 8 ms ago; otherwise retain one deadline at that paint time plus 8 ms. |
| More output arrives before that deadline | Parse it immediately. Keep the original deadline; new bytes cannot keep moving it forward. |
| Output stops with damage pending | Include the pending deadline in the existing poll timeout so the final frame is still drawn. |
| First frame, force, Expose or resize | Preserve the existing immediate update path, subject to an active synchronized-output hold. Expose can restore the retained committed pixmap. |
| Local input or authorized broker input is sent | Mark input pending. The next nonempty PTY read gets repaint priority. An intervening force paint of old content cannot consume this priority. |
| A synchronized-output hold ends | Retain a commit notification until damage is consumed; render it urgently when no newer hold is active. |
| Synchronized output remains active | Suppress ordinary redraw deadlines. Preserve the existing 150 ms hold timeout; repeated begin does not restart it. |
| No damage and no other existing deadline | Add no timer and no periodic wakeup. |

The constant is named `OUTPUT_REPAINT_BUDGET_MS`. It is a batching budget, not a
monitor refresh rate or a hard upper bound on input-to-display latency. Rendering,
OS scheduling and display-server work can exceed it. Input priority applies to
the next nonempty read after input, rather than identifying an application's
semantic response. DSR and other protocol replies do not mark all output as
interactive.

The change adds a timestamp and two flags per Linux UI, with no new dependency or
heap allocation in the scheduler. The shared terminal core adds a sticky
synchronized-output commit bit, including timeout and reset handling. The
[deterministic scheduler tests](../../tests/repaint_schedule.rs) exercise the
production scheduler directly.

## Why redraw submission was investigated

An earlier, isolated diagnostic intercepted three X11 functions in the verified
broker process of the old binary. Sampling with `perf record` was denied in this
environment, so no sampled CPU hotspot claim is made. The interceptor used
inclusive monotonic wall time and only the owned broker PID contributed counters.

| Function | Calls | Inclusive elapsed time |
| --- | ---: | ---: |
| `XftDrawStringUtf8` | 235,501 | 0.192356211 s |
| `XCopyArea` | 116,127 | 0.005235910 s |
| `XFlush` | 3,646 | 2.872329697 s |

The counters were enabled for **3.929628886 s**, including the final paint tail.
Thus `XFlush` occupied **73.1% of that wall interval**, including blocking and
descheduling. This is not a CPU percentage. Function times are inclusive and
cannot generally be summed; the observed nested-call counters were zero. A
single uninstrumented/instrumented pair took 3.375412448/3.879183265 s for the timed
workload. Its 14.9% elapsed increase combines instrumentation and host variation,
so it is not an isolated estimate of instrumentation overhead.

This evidence justified testing fewer submissions. It does not by itself prove
which kernel, server or scheduling work accounts for the waits. The A/B below
uses neither the interceptor nor `LD_PRELOAD`.

## Immutable measurement identities

Collection ran from 00:50:15.161140 to 00:50:28.685791 UTC on 2026-11-18.
The worktree was clean at source commit `60c672a` before and after collection.
Both executable hashes were checked before the runs and again after them.

| Variant | Source commit | Executable SHA-256 |
| --- | --- | --- |
| Before | `2dc6769b795e407d8fbecc2a305f1045868863a5` | `f73d7b66b8b081244e4b4ad9a302271e4d3778d456bff8e615cc03ae8ebcabf5` |
| After | `60c672aa6ee6dc79ba44182f1dd6079feced9a5c` | `786fec62515d50129be2216ce587b6fbda297368298922acad741f4a597f1c7a` |

The corresponding executable sizes were 677,088 and 677,328 bytes. This is file
size, not RSS. No new idle-memory result is inferred from it.

The retained collector has SHA-256
`aa2284ade3565ca55ba1258159a3a6174fc388e30da9c3f1e2a8882053f8b6a8`.
The unmodified A/B JSON has SHA-256
`cba7707c6f86e932db87b8233d972ff18e1985a7abe7d649bd59fad16760a18e`.
Its helper hashes include the ownership-checked Xvfb implementation,
`9baf8e2d731be49ed57eefbaa48e15ba30fe581839d0ad1738a5cc4a19ceeb0a`.
Historical commit identities remain the measured sources even if the change is
later integrated under a different commit.

## Workload and results

Each run started a new shared broker, its normal waiting launcher and one visible
120×32 terminal. Each display was a new owned Xvfb at 1600×1000×24 and 96 DPI.
The font was `monospace:size=11`, padding 16 and configured scrollback 2,000.
The payload overwrites cells using CUP and SGR without filling history.

After one warmup payload, the PTY child sent **64 × 1,050,937 bytes**
(67,259,968 bytes per measured run), with a DSR barrier after each payload.
Payload SHA-256:
`76376cc29e434d25d2fd58b984b231d6ad3efae2c7a9b0d8c87e762d12427f29`.
Throughput is bytes divided by the child's write-and-DSR elapsed interval.
DSR confirms logical terminal progress; it does not prove that a frame has been
presented.

The run order was before, after, after, before, before, after. All six accepted
samples are retained:

| Run | Variant | PTY throughput, MiB/s ↑ | Fresh-marker polling interval, ms ↓ | Private display | Xvfb PID | Broker PID |
| --- | --- | ---: | ---: | --- | ---: | ---: |
| 1 | Before | 20.6752 | 0.1708 | `:1369` | 3460177 | 3460197 |
| 2 | After | 133.0077 | 8.3979 | `:15275` | 3460666 | 3460711 |
| 3 | After | 130.9658 | 2.3986 | `:1056` | 3461404 | 3461425 |
| 4 | Before | 18.7654 | 0.0881 | `:13471` | 3461549 | 3461614 |
| 5 | Before | 18.8842 | 0.1139 | `:10247` | 3462289 | 3462297 |
| 6 | After | 278.5728 | 2.7314 | `:6980` | 3462784 | 3462792 |
| Median | Before | **18.8842** | — | — | — | — |
| Median | After | **133.0077** | — | — | — | — |

The ratio of medians is **7.0433** for this collection. The third after sample is
substantially higher than the other two; it has not been removed or replaced.
These short runs did not pin a CPU core or control CPU frequency. All observed
after samples exceed all before samples, but the spread prevents treating this
ratio as a universal performance guarantee.

The marker interval begins when the parent starts pixel polling, after it has
received the child's completion message and read geometry. It includes XGetImage
round trips and 1 ms polling sleeps, and can omit earlier work between the child's
write and the start of polling. It is an observation interval on Xvfb, not an
end-to-end latency measurement.

### Final pixels were independently checked

After the timed stream, the child writes an untimed, unique blue marker on row 1
and waits on a separate control socket. The collector polls actual X11 window
pixels without sending another PTY request or injecting input/Expose events.
It captures the whole window only after seeing the new marker; the post-capture
drain happens afterwards. A stale warmup frame therefore cannot pass this check.

All six final images were 1112×768 pixels and had identical hashes:

- RGB: `59db17fc924b6bb823912861b8d38dd969175a41b815cb832caa3188caf2f94e`.
- PNG: `c2c778c9b2a237f67082cc1ca47f5dee50fa3e475c2e9692b82d4b0bb74fe039`.

The actual candidate image was also inspected visually: the blue final marker,
complete text rows, dark background and padding are present without clipping.
The new two-session GUI suite's interaction image was inspected as well.

[Actual final window from run 6; all six final images have identical bytes（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view)

### Host pressure and cleanup

The preflight load1 was 3.2241 on 24 logical CPUs, with 2,870,444,032 bytes
available memory and 18,644,541,440 bytes of host swap already used.
At run-start/end samples, load1 ranged from 3.1680 to 4.1157, available memory
from 2,487,296,000 to 3,132,452,864 bytes, and host swap used from
18,615,066,624 to 18,825,863,168 bytes. Full PSI and memory snapshots are in the
raw JSON. Host swap is not a measurement of ztty process swap or RSS.

The existing comparison-pressure thresholds were used unchanged. Every start
was accepted and all end flags were empty. This records acceptable conditions
under that policy, not a claim that the host was idle or unswapped. Other project
builds and GUI runs were paused during the collection; unrelated user tasks were
not altered.

Each display's lease, lock PID, listener ownership and peer credentials were
verified. Broker executable hashes, start times, owned-child relationships and
markers were recorded. All six runs ended with no remaining known owned PID,
socket, PID lock or display lease. Cleanup was limited to the owned processes.

## Regression acceptance

The following checks passed with the final executable hash above. The initial
seven-case GUI run used an earlier candidate filename; the final rebuild was
byte-identical, and this equivalence is recorded in the evidence.

| Verification | Result |
| --- | --- |
| `cargo test --offline --locked -p zterm --lib` | 70 passed, including split-stream commit notification, same-feed begin/end, timeout and RIS regressions |
| `cargo test --offline --locked --test repaint_schedule` | 6 passed: fixed deadline, quiet final frame, urgent update, pending input across old-frame redraw, sync and no fake interaction |
| `cargo clippy --offline --locked --workspace --all-targets -- -D warnings` | Passed |
| `cargo fmt --all --check` | Passed |
| `cargo check --offline --locked -p zterm --target x86_64-unknown-none` | Passed; existing upstream `zune-png 0.5.2` SIMD future-incompatibility warning retained |
| `cargo build --offline --locked --release` | Passed |
| `tests/repaint_gui.py` with the fixed candidate | 7 actual PTY/XTEST/X11 checks passed |
| `tests/linux_native.py` with clean `60c672a` | Four suites passed: workspaces 10, daily workflow 14, Sixel 10 and input 8 checks |

Cargo builds used `CARGO_BUILD_JOBS=1` and the inherited global serial rustc
wrapper. No wrapper bypass or parallel project build was introduced.

The new GUI suite verifies a final frame after output becomes quiet, shared-broker
fairness, real XTEST echo and authorized broker input during a competing flood,
pending input across an old-frame Expose, explicit synchronized release, quiet
150 ms timeout and begin/end within one small write. During its 350 ms idle
observation, the broker's voluntary and involuntary context-switch deltas were
both zero. This short check supports the absence of an added periodic timer; it
is not an energy measurement.

Observed private-Xvfb intervals included 5.654 ms for the quiet final frame,
1.357 ms for XTEST echo while the other session flooded, 0.289 ms for a broker
write during the flood, and 151.898 ms from the first synchronized begin to the
timeout frame. The deterministic tests establish the scheduling decisions; the
GUI checks establish visible behavior with scheduling-tolerant thresholds.
These numbers are not physical keyboard-to-display latency.

## Protocol review and remaining scope

The official [Kitty performance documentation](https://sw.kovidgoyal.net/kitty/performance/#throughput)
explains that render-enabled throughput is sensitive to how much input is
batched before drawing. This is why the report pairs logical throughput with a
fresh final-frame check and retains separate fairness tests. The result is a
controlled comparison of two ztty binaries, not a new cross-terminal ranking.

Ghostty's official [render-hold guidance](https://lib.ghostty.org/vt/guide-rendering.html#render-holds)
describes retaining committed content during mode 2026, ending abandoned holds
with a timeout, and not extending the deadline on a repeated begin. Those cases
have direct regressions here. Its suggested per-hold boundary snapshot would also
preserve an intermediate completed frame when a release and another begin occur
within one feed. **ztty does not yet implement that snapshot behavior**: it keeps
the last painted pixmap and waits while a newer hold is active. The sticky
notification fixes missed end-of-hold notifications without claiming full
equivalence to Ghostty's boundary snapshots. ztty retains its existing 150 ms
policy; that value is not asserted to be required by the protocol.

This patch and collection leave the following work open:

- History-view output still uses the existing force path and can bypass the new
  coalescing budget. No history-view speedup is claimed.
- Full-history, Unicode, images, hidden sessions, multiple terminals and other
  PTY payloads were not remeasured in this A/B.
- Native Wayland, hardware GPU memory, first presented frame and physical
  input-to-display latency were not measured.
- This acceptance ran on Linux x86_64. It does not replace real Linux ARM64,
  macOS or Windows acceptance.
- Idle RSS was not remeasured. The earlier guarded single-terminal result remains
  22.02 MiB; the below-10-MiB goal is still unmet.
- The existing [three-application comparison](guarded-idle-1-10-2026-11-18.md)
  keeps its original source, payload aggregation, counts and SVG. Its throughput
  values must not be combined with this newer one-window A/B.

## Reproduction and retained evidence

The collector is deliberately pinned to the exact source and binary hashes above.
It refuses a dirty source tree, a different hash, an existing output directory or
unacceptable pressure. It creates its own guarded display and private session.
Requirements are Linux, Xvfb, Python, python-xlib and Pillow, plus the normal ztty
runtime libraries. The recorded environment was Linux 7.3.4 x86_64/glibc 2.44,
Python 3.14.10, python-xlib 0.33 and Pillow 13.0.0.

The complete evidence archive is named
`ztty-repaint-evidence-2026-11-18.tar.gz`. It contains both fixed binaries, a
source bundle retaining the original `60c672a` commit, raw runs, all screenshots,
the exact collector, diagnostic sources/counters, command logs and a hash
inventory. [Download the verified archive](https://drive.google.com/file/d/1SULsM0Y8e1t-MfV8nb-VKkw1e76-9Uwd/view).
Its SHA-256 is
`093ab9f08017c374e2e6b6c51fbe35ccdd389ff784a3e4045c5c372d4b3fbf01`
and its size is 1,411,169 bytes.

With that archive extracted to an absolute directory `REPAINT_EVIDENCE`, run
from an existing ztty clone containing the public base commit:

```sh
# Set these to fresh absolute paths on the test machine.
REPAINT_EVIDENCE=/absolute/path/ztty-repaint-evidence-2026-11-18
REPAINT_REPRO=/absolute/path/ztty-repaint-repro

git bundle verify "$REPAINT_EVIDENCE/source.bundle"
git fetch "$REPAINT_EVIDENCE/source.bundle" refs/heads/agent/bounded-repaint-20261118
git worktree add --detach "$REPAINT_REPRO" 60c672aa6ee6dc79ba44182f1dd6079feced9a5c
mkdir -p "$REPAINT_REPRO/artifacts"
cp "$REPAINT_EVIDENCE/repaint_ab.py" "$REPAINT_REPRO/artifacts/repaint_ab.py"
python3 -B "$REPAINT_REPRO/artifacts/repaint_ab.py" \
  --before "$REPAINT_EVIDENCE/binaries/before-ztty" \
  --after "$REPAINT_EVIDENCE/binaries/after-ztty" \
  --output "$REPAINT_REPRO/artifacts/repaint-ab-reproduced"
```

Use a fresh output path for each deliberate rerun. Different runtimes or font
selection can change pixels and timings. The archive's immutable binaries make
the original comparison rerunnable; rebuilding with a different compiler may
change their hashes and is a new experiment, not a silent replacement.

The visible-behavior tests can also run directly from a source checkout:

```sh
CARGO_BUILD_JOBS=1 cargo test --offline --locked -p zterm --lib
CARGO_BUILD_JOBS=1 cargo test --offline --locked --test repaint_schedule
CARGO_BUILD_JOBS=1 cargo build --offline --locked --release
python3 -B tests/repaint_gui.py --binary target/release/ztty \
  --output artifacts/repaint-gui.json --screenshots artifacts/repaint-gui
```

## Integration with the shared session library

The subsequent integration keeps the A/B's original binaries and results intact.
The current shared-core candidate passes 208 regular Rust tests, eight real
PTY core checks, strict Clippy, release/no_std auditing, MCP and bare-metal
portable-library checks. The known zune-png future compatibility warning remains.
All five actual Xvfb suites pass: workspace 10, daily 15, Sixel 10,
input eight groups with 27 vectors, and repaint seven checks.
[Exact integrated source/binary hashes and reports](../measurements/repaint-integrated-2026-11-18.json)
distinguish this candidate from the measured A/B source.

The formal Linux candidate gate now requires the repaint suite on both native
architectures, preserves its nested screenshots, and derives its required suite
count from the explicit case list. The ten native-gate packaging/failure
contracts pass. The forthcoming ARM64 execution remains necessary; these local
results do not claim that platform has already accepted the new scheduler.
