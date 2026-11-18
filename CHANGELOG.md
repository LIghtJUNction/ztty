# Changelog

## 0.3.0 — 2026-11-18

Unreleased: tag `v0.3.0` is fixed at `9b5497d`. Its formal release attempt failed
the Windows ARM rapid-resize gate; publication was skipped and no device
installation was performed. A repaired release will use a new tag. Verification
entries below retain their separate source checkpoints.

### Added

- Add a temporary Linux **Back to live ↓** control when viewing scrollback.
  Mark arriving PTY output, keep the inspected rows anchored, and clear the
  indicator on returning to the bottom. Preserve search/palette input modes,
  observing-view ownership and PTY dimensions; withhold the control's mouse
  gesture from the application. Mouse-wheel and Shift+PageUp/PageDown browsing
  remain available. [Behavior and limits](docs/daily-workflow.md#returning-to-live-output).

- Add the Linux `sessions` action and default `Ctrl+Shift+S` session switcher.
  Filter names, titles and URIs; select with Up/Down and Enter. Focus existing
  tabs, panes and windows in place, and explicitly attach sessions without a
  window while preserving PTYs, input ownership and observing views.

- Let the Linux command palette match multiple words across action names,
  labels and shortcuts. Display effective custom/default bindings, and keep
  empty-result queries open when Enter is pressed.

- Add `install.sh --from-binary PATH` for local native builds and rollback from
  a saved regular executable. Route `make install` through the installer, with
  `PREFIX` and `DESTDIR` support and configurable binary/launcher directories.

- Add opt-in `--http-response sse` for MCP POST responses, with prompt headers,
  five-second keepalive comments and a final JSON-RPC event. Preserve the default
  JSON mode, request cancellation, role/session isolation and bounded buffers;
  verify both forms through real PTYs and integrate SSE checks into the local gate.

- Add bounded `session_read.wait_ms` over stdio and HTTP, with concurrent tool
  requests, cancellation, explicit timeout results and PTY-instance binding.
  Parked reads preserve independent byte cursors and incomplete UTF-8 suffixes.

- Add optional MCP session_read identity guards and output_eof, so reconnecting observers can reject a replacement PTY and distinguish child exit from reader termination without new per-session state.

- Add no_std Darwin PTY creation through a fresh posix_spawn helper, with controlling-terminal setup, exact argv/environment, close-on-exec descriptor isolation and bounded error handoff; native SDK/concurrent-spawn acceptance is now a mandatory Mac candidate gate.

- Allow build-only native candidates to select Linux, macOS or Windows while keeping both architectures, pinned provenance and mandatory all-platform publication gates.

- Export the common action, JSON, pane-layout and workspace-state modules from the no_std library; Linux and regression tests consume the same implementations.

- Export a shared no_std session library and route Windows/macOS PTY reads,
  bounded writes and resize commits through it.

- Add native Windows tabs and split panes, with independent ConPTY children,
  pane focus, layout changes and preserved frames during synchronized output.
  Expanded native workspace acceptance is recorded separately from the earlier
  single-pane image and input gate.

- Shared no_std PTY pump and bounded input queue, now used by the Linux broker, with per-read history anchors and PTY-first resize commits.

- Portable alloc-only input ownership state, used by the Linux broker, with same-owner queue preservation and explicit local observer recovery covered by real PTY and GUI regressions.
- Native macOS and Windows startup `--cwd`, with Unicode paths, command lookup in the selected directory and preserved relative config semantics; Darwin child PWD follows the physical directory.
- macOS WindowServer capture and owned-window resize/minimize acceptance, retaining existing PTY checks and recording Screen Recording/Accessibility blockers explicitly.
- Three-run guarded 1/10-window benchmark evidence and README charts, including measured RSS/PSS/Swap, source hashes, the unmet 10 MiB RSS target and the PTY throughput gap.
- Linux standalone `--observe` command windows and the `ztty-observer` external-terminal adapter; `close NAME` follows existing input ownership authorization.
- Linux x86_64/ARM64 release gates require six real X11/PTY suites, including
  the session switcher, with source, executable and display ownership evidence;
  Windows ARM64 must execute natively.
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

- Preserve Windows pane history and selection for keyboard events that encode
  no terminal input, including the Shift release after Shift+PageUp and silent
  modifiers used to switch focus. Actual encoded input still returns to live
  output and clears the selection.

- Give Windows ConPTY children their own console standard handles when ztty's
  stdin, stdout or stderr is redirected, keeping the child's input/output
  inside its terminal instead of inheriting the host's redirected streams.

- Join soft-wrapped rows during Linux history search and selection copying,
  while preserving explicit line breaks. Track matches across physical rows,
  copy complete matched cells beyond the viewport, and refresh stale active
  search hits before copying after output or resize.

- Keep Caps Lock, Num Lock and Scroll Lock silent in legacy keyboard mode.
  Preserve Num-Lock-off keypad Insert and lock-key reports requested through
  the enhanced Kitty keyboard protocol.

- Stage, checksum and validate an installer candidate before atomically
  replacing the executable. Retain one previous executable or original symlink
  for rollback, preserve that backup on an identical reinstall, and write a
  desktop launcher using the absolute installed path. Running processes keep
  their loaded executable.

- Point the AUR package's desktop launcher at `/usr/bin/ztty`, and align
  installer/template `StartupWMClass` with the native X11 class `Ztty`.

- Defer and coalesce Windows resize messages received during nested callbacks.
  Preserve the mutable-state guard, reread the latest client dimensions from
  the outer loop, and retain requests arriving during a resize for the next turn.

- Send a real Enter key when launching Windows workspace test children; keep
  Unicode text packets for command text and read UTF-8 evidence explicitly.

- Match equivalent numeric MCP request IDs exactly for pending-request rejection
  and cancellation over stdio and HTTP. Preserve original response tokens,
  string/number separation and arbitrary-size integers without float rounding
  or exponent expansion.

- Repair Bash integration syntax, preserve complete PROMPT_COMMAND strings/arrays and avoid duplicate PS0 execution markers in nested shells. Check syntax and each source result independently.

- Avoid clearing Unicode target cells immediately before overwriting them; preserve wide-cell boundary repair, insertion, metadata and wrapping, with five new core regressions.
- Integrate main-branch throughput optimizations with production workspaces, palette provenance and X11 overlays; fix independent benchmark broker selection and runtime A/B window counts.

- Make macOS helper activation-policy setup idempotent and retain before/after
  state before failure; the observed prohibited policy remains required before
  WindowServer acceptance.

- Call the macOS helper activation-policy method correctly after both native
  compilers rejected property syntax, retaining the earlier failure evidence.

- Drain Darwin PTY probe output while waiting for child exit so a full output
  buffer cannot stall the threaded-spawn test; retain probe phase and child-PID
  evidence when a native attempt fails.

- Check a Darwin helper's execute access using effective user/group identity
  before opening a PTY or spawning it. Missing and non-executable helper paths
  return their original errors without creating a child; the access check does
  not make filesystem changes between checking and execution atomic.

- Use sRGB window backing on macOS so native RGB image samples retain their
  protocol colors through the captured window surface.

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

- Record the failed formal `v0.3.0` attempt at source `9b5497d` in
  [run 44309412482](https://github.com/LIghtJUNction/ztty/actions/runs/44309412482).
  Verify passed 271 ordinary Rust tests, 6 distinct doctests, 8 separate
  SessionCore checks and 12 GUI smoke groups. Each Linux architecture passed
  six GUI suites / 67 groups / 27 PNG captures; each Mac architecture passed
  8 spawn and 24 native checks. Windows x64 passed 31 base and 19 workspace
  groups. Windows ARM completed 23 base checks, then failed
  `resize_restores_visible_image`: the 10-second wait ended at RGB `[20,20,20]`
  instead of `[255,0,0]`. Workspace checks were not reached and cleanup passed.
  Two fresh traced rapid/staged diagnostic processes each passed 31 checks,
  without changing the original failure or establishing its cause. Publish was
  skipped and the by-tag Release API returned 404. The tag remains unchanged;
  no Release assets were published or installed.
  [Formal evidence](docs/experience-release-2026-11-18.md#54-正式-v030-门禁失败).

- Accept Windows source `3418b82` in
  [run 44306719420](https://github.com/LIghtJUNction/ztty/actions/runs/44306719420):
  native x64 and ARM64 each passed all 31 base checks and 19 workspace groups,
  with owned-process and desktop cleanup. The original rapid resize path ran
  without runtime tracing or added resize acknowledgements. Both architectures
  preserved blue history and selection through silent modifiers/focus changes,
  and actual Enter input returned to live output. This is an earlier build-only
  candidate; its acceptance does not override the later formal ARM failure.

- Record the intervening Windows candidates separately:
  [source `8a2f4be`, run 44303751871](https://github.com/LIghtJUNction/ztty/actions/runs/44303751871)
  failed after 7 checks when redirected host streams exposed invalid child
  console handles. After the `e334cf7` stdio fix,
  [run 44305368950](https://github.com/LIghtJUNction/ztty/actions/runs/44305368950)
  passed all 31 base checks on both architectures but failed pane history
  after 5 workspace groups. Neither failed run is promoted by later acceptance.

- Preserve the failed Windows-only retry at source `dc43870` in
  [run 44300272576](https://github.com/LIghtJUNction/ztty/actions/runs/44300272576).
  Both native architectures completed 22 of 30 base checks and 12 visible-pixel
  checks before failing `resize_restores_visible_image`. Workspace checks were
  not reached. Application, Job and desktop cleanup passed on both runners;
  that source did not pass native acceptance.

- Record `ZTTY_BARE_METAL=1 make check` at Linux source `8a2f4be`: 271 ordinary
  Rust tests, 6 doctests and 8 separate SessionCore PTY checks passed, together
  with strict workspace Clippy, release build, installer, shell, MCP and no_std
  gates. [Fixed-source result and full log](docs/experience-release-2026-11-18.md)
  retain Darwin skips and the dependency's future-compatibility warning.

- Extend the Linux daily-workflow harness to 18 groups with real mouse return,
  new-output anchoring, search/palette coexistence and read-only control checks.
  All 18 groups passed on the development device at harness source `aef55d9`,
  using the `3730b28` executable; the earlier candidate's passing counts
  do not cover this addition.

- Record source `2ac46e2` in [candidate run 44298970562](https://github.com/LIghtJUNction/ztty/actions/runs/44298970562):
  both macOS architectures passed 8 threaded-spawn checks, 24 native checks,
  13 stages and 10 PNG captures each, with application exit 0 and no owned
  processes remaining. Linux x86_64/ARM64 passed all six GUI suites. Windows
  x64 failed image restoration; ARM64 passed the initial 30 native checks but
  failed the new workspace child launch. These results do not establish
  final-tag acceptance or installation.

- Record source `a045871` on macOS Apple Silicon: all 8 threaded-spawn checks,
  24 native checks, 13 gate stages and 10 PNG captures passed, with no owned
  processes left behind. The same run's Intel job failed in threaded-spawn
  acceptance before its native window gate. [Candidate run 44293476047](https://github.com/LIghtJUNction/ztty/actions/runs/44293476047)
  is scoped to macOS and does not establish final-tag acceptance; IME/keyboard
  input and physical display latency remain unmeasured.

- Preserve the failed `bfd2732` [native retry](https://github.com/LIghtJUNction/ztty/actions/runs/44295578969):
  Intel passed all 8 threaded-spawn checks but failed the first native RGB check
  with red `[255, 0, 3]` against `[255, 0, 0]`; ARM failed a child-cleanup
  assertion after its concurrent probes, before window acceptance. Neither
  architecture produced an accepted build in that retry.

- Validate the shell/resume candidate with 225 Rust tests, 6 doctests, four real Shell integrations, 11 Bash PTY checks, 17 workspace GUI checks and three MCP suites. The Linux local gate now runs Bash PTY acceptance; see [the source-bound report](docs/shell-resume-2026-11-18.md).

- Record the 2026-11-18 integrated candidate: 220 Rust unit/integration tests, 6 doctests, 61 real Linux GUI checks, 3 MCP suites and 38 Python benchmark regressions; retain fixed-CPU and unpinned Unicode microbenchmarks with all samples and scope limits. See [the integration report](docs/integration-unicode-2026-11-18.md).

- The integrated shared-core/repaint candidate passes 208 regular Rust tests, eight real PTY checks and five Xvfb suites; native Linux candidates now also require the repaint pixel/fairness/idle checks.

- The earlier shared-core checkpoint passed eight single-threaded real-PTY checks,
  the integrated Rust/Clippy/MCP/no_std gates, 15 daily GUI groups and 10 workspace
  GUI groups.

- Fourth source-pinned native run accepts Linux x86_64/ARM64 and Windows x86_64 (30 checks plus cleanup); preserves Windows ARM64 occlusion and both macOS helper initialization failures with all artifact hashes.
- Bounded repaint passes 70 zterm and 6 scheduling tests, 7 real X11/PTY regressions and the four existing Linux suites (42 checks). Three fixed-binary A/B pairs measured median logical throughput 18.88 → 133.01 MiB/s with identical final pixels; scope, variation and the pending per-hold snapshot behavior are recorded in [the report](docs/benchmarks/repaint-ab-2026-11-18.md).
- Earlier local gates passed 170 Rust unit/integration/documentation tests, strict Clippy, MCP transport/session/security tests and bare-metal checks; the zune-png bare-metal SIMD future-compatibility warning is retained in the evidence.
- The earlier real ssmcp relay integration passed three native commands and ten checks, including window reuse and reopening; its daily Linux GUI harness had fourteen checks.
- Windows native gates exercise six Sixel pixel/recovery groups and save
  owned-window PNG evidence; source `7a1d06b` accepted both native x64 and ARM64.
  Each release still requires those gates against its own tagged source.
- Twenty-seven real X11/PTY vectors cover legacy keyboard bytes, four wheel directions, extra buttons, press ownership, focus transitions, X10 mode and custom bindings. Button 11 uses explicitly recorded synthetic X11 events because the virtual pointer exposes ten buttons.
- Six layout tests and an isolated real X11/PTY acceptance script cover geometry, actual keyboard routing, hidden output, detach/reattach and ownership.
- Windows workspace acceptance passed at `3418b82`; macOS workspace UI and
  multi-view GUI observation remain pending.
- Native macOS/Windows search, command palette and shared MCP runtime are not yet implemented.
