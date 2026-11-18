# Changelog

## Unreleased

### Added

- Add no_std Darwin PTY creation through a fresh posix_spawn helper, with controlling-terminal setup, exact argv/environment, close-on-exec descriptor isolation and bounded error handoff; native SDK/concurrent-spawn acceptance is now a mandatory Mac candidate gate.

- Allow build-only native candidates to select Linux, macOS or Windows while keeping both architectures, pinned provenance and mandatory all-platform publication gates.

- Export the common action, JSON, pane-layout and workspace-state modules from the no_std library; Linux and regression tests consume the same implementations.

- Export a shared no_std session library and route Windows/macOS PTY reads, bounded writes and resize commits through it; Windows x64/ARM64 now pass the native 30-check gate; macOS acceptance is pending.

- Shared no_std PTY pump and bounded input queue, now used by the Linux broker, with per-read history anchors and PTY-first resize commits.

- Portable alloc-only input ownership state, used by the Linux broker, with same-owner queue preservation and explicit local observer recovery covered by real PTY and GUI regressions.
- Native macOS and Windows startup `--cwd`, with Unicode paths, command lookup in the selected directory and preserved relative config semantics; Darwin child PWD follows the physical directory.
- macOS WindowServer capture and owned-window resize/minimize acceptance, retaining existing PTY checks and recording Screen Recording/Accessibility blockers explicitly.
- Three-run guarded 1/10-window benchmark evidence and README charts, including measured RSS/PSS/Swap, source hashes, the unmet 10 MiB RSS target and the PTY throughput gap.
- Linux standalone `--observe` command windows and the `ztty-observer` external-terminal adapter; `close NAME` follows existing input ownership authorization.
- Linux x86_64/ARM64 release gates now require five real X11/PTY suites with source, executable and display ownership evidence; Windows ARM64 must execute natively.
- Versioned Linux workspace snapshots restore live tabs, split ratios, focus and geometry without replacing PTYs; bounded schema/identity checks reject invalid layouts before UI mutation.

- Linux workspace tabs and horizontal/vertical split panes in the shared broker, with pane focus, swap, resizing, maximize/restore and moves between native windows.
- Stable session IDs, creation timestamps, filtered listing and rename that preserves existing session URIs.
- Explicit local detach, observe attach and input ownership with deliberate takeover.
- Hidden-pane release of rebuildable Xrender image uploads and retained X pixmaps.
- Linux UTF-8 history search with visible match highlighting, bounded query storage and forward/backward navigation.
- Temporary command palette, named key/mouse actions, live theme/font/binding reload and font-size shortcuts.
- Bounded OSC 133 command records, command navigation/copy, CLI output and observer-safe MCP command reads.
- Opt-in Bash, Zsh, Fish and Nushell integration scripts and the `shell-integration` command.
- Read-only MCP roles, separate observer/operator tokens, exclusive trusted actor identities and stricter HTTP/JSON framing.
- Reproducible independent/shared 1/5/10/20 benchmark tooling, workload checks, retained raw measurements and SVG generation.

### Fixed

- Avoid clearing Unicode target cells immediately before overwriting them; preserve wide-cell boundary repair, insertion, metadata and wrapping, with five new core regressions.
- Integrate main-branch throughput optimizations with production workspaces, palette provenance and X11 overlays; fix independent benchmark broker selection and runtime A/B window counts.

- Make macOS helper activation-policy setup idempotent and retain before/after state before failure; the actual prohibited policy remains required. Native execution is pending.

- Call the macOS helper activation-policy method correctly after both native compilers rejected property syntax; preserve the fifth-run failure evidence and require a native rerun.

- Isolate the Windows CI pixel gate on its own visible desktop and owned Job, with explicit handle inheritance, secure-desktop refusal and verified restoration; verified with all 30 checks and desktop restoration on native Windows x64 and ARM64.

- Initialize the macOS pixel helper through NSApplication.shared and retain real helper exit/signal and capture-stage evidence before JSON decoding.

- Bounded shared output cursors retain a split UTF-8 suffix even after earlier malformed bytes; a gated real PTY regression proves the prior loss and the corrected byte offsets.
- Native acceptance now disposes owned Windows processes before their cwd, preserves original failures, and uniquely selects the titled macOS terminal before pinning WindowID; actual third-run Linux ARM64 acceptance and remaining failures are retained.
- Linux links the real system unwinder after Rust archives so GNU ld --as-needed cannot discard it before debug alloc cleanup references.
- Windows native gates search for a visible location for their own window and record covering-window identities, while retaining the pixel ownership checks and original assertions.
- Linux continuous PTY output now coalesces ordinary redraws behind a fixed 8 ms scheduling budget, preserves prompt parsing and four-read session fairness, commits quiet final damage, and prioritizes interaction and synchronized-output release without adding idle polling.
- Linux sessions save cwd with portable O_PATH/CLOEXEC flags, fixing ARM64 startup and supporting search-only original directories; save and enter failures retain their own errno.
- Windows native pixel gates verify on-screen ownership and clipping, record resize-stage PNGs and report the actual final RGB without relaxing color or timeout requirements.
- Custom benchmark charts now link their actual raw-data path; renderer-only updates preserve collection identity and historical evidence.
- Closing a newly launched observer retires its own viewer child, allowing the next ssmcp task to reopen visibly; attaching to an existing session still preserves that session on close.
- Config reload reconnects without losing a directory created before its ancestor watch is installed; deterministic real inotify regressions prove the old failure and the repaired behavior.
- Native Linux evidence is archived and read back before staging cleanup, preserving the flat Release checksum contract; incomplete gates cannot produce accepted artifacts.
- Benchmark reports now pin the isolated Xvfb helper hash and preserve collection-time identity when regenerating charts.
- Linux input preserves horizontal wheels and extra mouse buttons 8–11; configured bindings support all eleven X11 buttons.
- Mouse releases follow the destination of their original press across Shift changes; focus and ownership transitions clear held input without cancelling a valid focus-loss clipboard transfer.
- Workspace restore retains live parent/child windows and retries PTY dimensions after resize errors; stale queued focus/geometry events no longer overwrite the restored layout.
- Saved layouts reject a replacement session that reuses a closed session's canonical URI.

- Closing an observing view preserves another operator's live PTY; observing an existing workspace does not steal focus.
- Ownership handoff cancels pending input and asynchronous clipboard transfers.
- Command navigation skips regions invalidated by a screen-width change.
- Config watches follow atomic file saves and replacement of the containing directory, and release unused watches.
- Runtime theme updates distinguish default, indexed and explicit RGB colors without enlarging the 28-byte cell.
- Large JSON object duplicate-key validation avoids quadratic work.

### Verification

- Record the 2026-11-18 integrated candidate: 220 Rust unit/integration tests, 6 doctests, 61 real Linux GUI checks, 3 MCP suites and 38 Python benchmark regressions; retain fixed-CPU and unpinned Unicode microbenchmarks with all samples and scope limits. See [the integration report](docs/integration-unicode-2026-11-18.md).

- The integrated shared-core/repaint candidate passes 208 regular Rust tests, eight real PTY checks and five Xvfb suites; native Linux candidates now also require the repaint pixel/fairness/idle checks.

- Shared core passes eight single-threaded real-PTY checks, the integrated Rust/Clippy/MCP/no_std gates, daily GUI 15 and workspace GUI 10.

- Fourth source-pinned native run accepts Linux x86_64/ARM64 and Windows x86_64 (30 checks plus cleanup); preserves Windows ARM64 occlusion and both macOS helper initialization failures with all artifact hashes.
- Bounded repaint passes 70 zterm and 6 scheduling tests, 7 real X11/PTY regressions and the four existing Linux suites (42 checks). Three fixed-binary A/B pairs measured median logical throughput 18.88 → 133.01 MiB/s with identical final pixels; scope, variation and the pending per-hold snapshot behavior are recorded in [the report](docs/benchmarks/repaint-ab-2026-11-18.md).
- Final local gates pass 170 Rust unit/integration/documentation tests, strict Clippy, MCP transport/session/security tests and bare-metal checks; the zune-png bare-metal SIMD future-compatibility warning is retained in the evidence.
- Real ssmcp relay integration passes three native commands and ten checks, including window reuse and reopening; daily Linux GUI coverage now has fourteen checks.
- Windows native gates now exercise six Sixel pixel/recovery groups and save owned-window PNG evidence; execution on x64 and ARM64 is still required.
- Twenty-seven real X11/PTY vectors cover legacy keyboard bytes, four wheel directions, extra buttons, press ownership, focus transitions, X10 mode and custom bindings. Button 11 uses explicitly recorded synthetic X11 events because the virtual pointer exposes ten buttons.
- Six layout tests and an isolated real X11/PTY acceptance script cover geometry, actual keyboard routing, hidden output, detach/reattach and ownership.
- Native macOS/Windows workspace UI and multi-view GUI observation remain pending.
- Native macOS/Windows search, command palette and shared MCP runtime are not yet implemented.
