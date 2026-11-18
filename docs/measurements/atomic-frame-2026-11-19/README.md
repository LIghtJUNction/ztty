# Complete-frame X11 presentation

Date: 2026-11-19

## Integrated with 0.3.1

The final source candidate `4f7872bcd78df67431074c7ed3265c1f28bbed93` merges the
0.3.1 mainline `cbcd480f861904a2265af52ab5ca8a5e19cebe9d` with the
frame fix. All normal terminal and overlay drawing paths remain inside the
single `finish_frame` call. Expose alone restores retained pixels directly.

The published equivalent is `cabeba6a7d74b36497e861febe5b7bb04d01c8ba`.
Its Git tree, `a04fa0107313894f6f57ec1532b960250b510985`, is identical to
the tested source tree. The source was merged through GitHub after the native
Git transport stopped connecting; the tested local commit is retained in the
evidence to keep the build provenance exact.

The incremental release build reused the existing target directory. Its exact
binary passed all **11 repaint checks** and the existing Linux input GUI suite:
**12 checks, 59 input vectors and 14 zoom measurements**. This covers
zoom/reset, non-US plus keys, custom bindings and Ctrl+Tab/Ctrl+Shift+Tab, with
no consumed key-release leakage. The first input-suite launch stopped before
any GUI scenario because its 5-second source-status read timed out; the source
read then completed in 0.063 seconds, and the unchanged suite passed.

[Integrated evidence](integrated-0.3.1.json) records source hashes, the exact
binary hash, copied regions and cleanup. A local **0.3.1-2** package was prepared
from that same binary without recompilation or stripping. Extraction verifies
byte identity. The [installation receipt](installation-0.3.1.json) records
the subsequent local installation: all 9 package files passed integrity checks,
the installed executable matches the GUI-tested hash, and all 3 preexisting
terminal processes and the user's launchers were preserved.

New launches normally reuse the shared broker, and that connection has no
executable-version handshake. At installation verification, no default broker
was listening in the common network namespace; the older 0.1.2 windows use the
earlier independent-window implementation. In that observed state, a new default
launch creates a broker from the newly installed executable. Existing windows
keep their old code. If a broker is already listening, an unused `ZTTY_SOCKET`
name starts an independent broker without closing existing sessions.

Binary SHA-256: `6739a4f1d535ada1885cafed39a808cbe667cc527f83f19769d664be5597de07`

Package SHA-256: `d8c2d70cc6e7e040853e8323e0c4de99b86a50eba684746302d3ed16ebe99d1c`

![Integrated scrollback frame](integrated-0.3.1-scroll.png)

## Original baseline verification

The Linux renderer now publishes one complete frame after painting the terminal
and its view overlays. The real-window regression passes all 11 checks.
The same test reproduces three visible copies in the previous renderer for
one synchronized update to three nonadjacent rows.

Source change: `4c391f123227b63c68299311ec6d4cc5518237cb`. Baseline: `868e841fc81f747283de40eed4956e52ca063aba`.
The exact binary hashes, source hashes, copy rectangles and cleanup results are
recorded in [evidence.json](evidence.json).

## Cause and change

The previous renderer copied each completed span of dirty rows from its retained
pixmap to the visible window while later rows were still being painted. Delaying
the final `XFlush` did not make those copies a transaction: Xlib may send buffered
requests earlier. Search and scrollback overlays also issued their own copies.

Painting now records the vertical bounds of damage. After the terminal, search
or command overlay, notice and scrollback prompt are complete, `finish_frame`
copies their union once. Expose still restores the retained frame immediately.
The DEC 2026 hold and 150 ms stalled-frame timeout are unchanged.

| Scenario | Observed visible copies | Copied height |
| --- | ---: | ---: |
| Old renderer, three nonadjacent rows | 3 | 23 px each |
| Fixed renderer, the same update | 1 | 736 px |
| Fixed renderer, one changed row | 1 | 23 px |
| Fixed renderer, terminal plus search | 1 | 768 px |
| Fixed renderer, scrollback plus return prompt | 1 | 768 px |

The union can include unchanged rows between separate damaged regions. Those
cells are not redrawn. A single-row update still copies only that row.

## Verification

Both binaries were built with `cargo build --locked --offline --release`
in separate worktrees. The final test harness was run against both artifacts.
The baseline intentionally fails the new one-copy assertion; the candidate
passes it and the existing quiet-output, idle, concurrent-input, Expose and
synchronized-output checks. Formatting and `git diff --check` also pass.

The test-only preload tracer records real `XCopyArea` calls without changing
their order, geometry or flushing. Independent X11 pixel reads verify the
visible result. The tracer is never linked into the shipped executable.
All test-owned processes and private displays were cleaned up.

Run the regression from the candidate source tree:

```sh
python3 tests/repaint_gui.py --binary ./target/release/ztty
```

![Scrollback and its return prompt are committed together](single-publish-scroll.png)

## Scope

This verifies software frame composition and X11 presentation on a private
display. It does not measure physical monitor scanout, compositor vblank timing
or a hardware-specific tearing guarantee. Applications that write a logical
frame in several PTY chunks should bracket it with synchronized output; the
ssmcp sender change is tracked separately. This report records source and GUI
verification. The separate installation receipt records the local deployment;
neither claims a hot upgrade of existing windows or physical scanout testing.
