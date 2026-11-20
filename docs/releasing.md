# Releasing ztty and its reusable libraries

## 0.5.2 candidate — 2026-11-20

This candidate starts from the fixed
[0.5.1 source](https://github.com/LIghtJUNction/ztty/commit/1631db8a02e8627d5d35ac0e1d4bef93475f45dc)
and adds [input-buffer reclamation and its evidence](https://github.com/LIghtJUNction/ztty/commit/79001d4af9414cea9b53a46efc7350a0161973cb).
Its runtime change releases queue allocations larger than 64 KiB after input
is fully sent or explicitly cancelled, while retaining small buffers for reuse.

The [native UI probe correction](https://github.com/LIghtJUNction/ztty/commit/b42c0d7e787476d760241fa92930d77bab28d675)
also waits for the outgoing tab to be unmapped as well as the incoming tab to
be mapped. Mapping an incoming view starts its complete-frame transition; it
does not prove the outgoing view has been hidden. The existing timeout,
external ButtonRelease and PTY checks remain required.

The [memory report](benchmarks/input-buffer-memory-20261120/report.zh-CN.md)
records queue capacity and Linux backpressure checks at its stated source.
It does not measure process RSS/PSS or throughput. Build, package verification
and native acceptance of this integrated 0.5.2 candidate are pending.

The source reuses the published libraries below and the 0.5.1 release gates.
Application publication requires the complete native workflow and
registry-package verification. Installation is checked separately against the
accepted release artifact.

The application pins all six locally sourced Project Z packages to exact
registry versions. [Cargo packaging](https://doc.rust-lang.org/cargo/commands/cargo-package.html)
removes local dependency paths and the source manifest's patch section. Direct
constraints for zgpui, zgraphics and zinput keep those registry versions aligned
with the source submodules, with default features disabled as before. Verify
the generated archive's lockfile and resolved features before publication; a
clean source commit and its source lockfile do not establish the packaged graph.

### Source and dependency order

| Stage | Package | Version | Required before publication or release |
| --- | --- | --- | --- |
| 1 | `zgpui` | `0.1.2` | Its independent repository's candidate checks |
| 1 | `zgraphics` | `0.2.0` | Package verification and existing registry dependencies |
| 1 | `zinput` | `0.1.3` | Its independent repository's release checks |
| 1 | `zpty` | `0.2.3` | Its independent repository's release checks |
| 1 | `ztty-plugin-api` | `0.1.0` | Package verification and `zplugin =0.3.1` |
| 2 | `zterm` | `0.5.2` | Published `zgpui 0.1.2`, `zgraphics 0.2.0` and `zinput 0.1.3` |
| 3 | `ztty` application | `0.5.2` | Published `zterm 0.5.2`, `zpty 0.2.3`, `ztty-plugin-api 0.1.0` and complete application gates |

`zgpui 0.1.2`, `zgraphics 0.2.0`, `zinput 0.1.3`, `zpty 0.2.3` and
`zterm 0.5.2` are published from their independent repositories. Reuse these
versions and the candidate's fixed submodule commits and lockfile.
`ztty-plugin-api 0.1.0` is also published. Its [publication and registry record](reports/plugin-api-registry-20261120/README.md)
records the earlier six isolated consumers and their combined consumer passed with
published `zterm 0.5.0`. Application publication is recorded separately after the full workflow succeeds.

[zgpui](https://github.com/LIghtJUNction/zgpui) is an independent repository,
included at `crates/zgpui` as a Git submodule pinned to
[`5b268da910639abd2fb8e581acb4f6eed50b7516`](https://github.com/LIghtJUNction/zgpui/commit/5b268da910639abd2fb8e581acb4f6eed50b7516).
Initialize all five library submodules with
`git submodule update --init --recursive` before building a fresh checkout.
`zgpui` replaces the former `ztty-ui` package; `zterm::ui` remains the public re-export and the
ZUI/1 wire format is retained. Its scene, layout and renderer contracts are
platform-independent. This application leaves zgpui features `std` and `native` disabled;
its existing platform hosts provide native windows and graphics.

### Candidate to release

1. Commit the versioned source, lockfile and submodule pin, then record the
   candidate SHA. Run the local gates and required native checks against that
   fixed source; any source change creates a new candidate.
2. Release changed `zgpui` versions through its own
   [release workflow](https://github.com/LIghtJUNction/zgpui/blob/main/.github/workflows/release.yml).
   Use `bump=none` for an already versioned candidate. A retry supplies the same full
   candidate SHA. The workflow requires `CARGO_REGISTRY_TOKEN`, validates that
   exact candidate, and checks any existing tag and archive before continuing.
3. Verify and publish changed independent libraries from their own repositories
   in the order above; this candidate reuses the published versions listed here.
   For a changed `ztty-plugin-api` version, publish from the root manifest path
   below; the current `0.1.0` is already published.
   Confirm registry versions and package contents before verifying consumers
   without workspace path overrides.
4. Run the [application Release workflow](../.github/workflows/release.yml) for
   `v0.5.2`. Formal publication requires Verify and all six native architecture
   gates on that tagged source, plus source and artifact readback. Retain the
   immutable tag and record published downloads separately from candidate
   artifacts.
5. From the same clean source, run `cargo package --locked`. Read the generated
   archive's `Cargo.lock` and require the six Project Z versions in the table above.
   Install from the unpacked crate with `--locked` in an isolated directory,
   verify registry-only dependency sources, and record the resolved features.
   After all formal native gates pass, run `cargo publish --locked`. Read back
   the registry archive, checksum, embedded source commit and dependency versions.
6. Install the accepted application artifact and record the installed version
   and executable hash. Publication alone does not establish device installation.

### API publication and registry verification

The native application gates build `ztty-plugin-api` through its local `path`
dependency. Passing those gates does not verify the registry package and does
not require the API to have been published first. For a future API version,
package and publish from its approved, clean candidate checkout:

```sh
cargo package --locked --manifest-path crates/ztty-plugin-api/Cargo.toml
cargo publish --locked --manifest-path crates/ztty-plugin-api/Cargo.toml
```

After all candidate dependency versions are available in the registry, run the
default verification for all six libraries and their combined consumer:

```sh
python3 tools/verify_registry.py --output artifacts/registry-consumer.json
```

This verification uses exact registry versions without local path overrides.
Record registry closure only when all six consumers and the combined consumer
pass. The verification tool does not publish crates.

## 0.5.1 validation record — 2026-11-20

The immutable `v0.5.1` tag points to source
[`1631db8`](https://github.com/LIghtJUNction/ztty/commit/1631db8a02e8627d5d35ac0e1d4bef93475f45dc).
Its 17 local validation stages completed, but the formal
[Release run 44706305817](https://github.com/LIghtJUNction/ztty/actions/runs/44706305817)
failed the GPU outside-release probe on both Linux architectures. The first
ARM attempt stopped during Rust toolchain installation; the second built
successfully and then reproduced the same probe failure. Windows, macOS and
Verify passed on that source. Publication was skipped and no GitHub Release
was created. The failed tag is preserved; 0.5.2 uses a new immutable tag and
must pass the complete workflow again.

The local 0.5.1 registry package also had a different dependency input from
the source checkout. Its clean `.cargo_vcs_info.json` names `1631db8`, but its
generated lockfile and installation resolved `zgpui 0.1.3` through the version
range allowed by published `zterm 0.5.2`. The source submodule and exact registry
consumer used `zgpui 0.1.2`. The local package and its installed X11/reload
checks passed; they do not establish a single shared dependency graph or
formal publication. The application's exact constraints and archive readback
requirements above address this difference for 0.5.2.

## Historical records: 0.4.1 and earlier

The records below retain their original dates, versions, commands and outcomes.
References to the current source or latest checks within those records apply
to their recorded date; they do not establish 0.5.0 acceptance.

Report date: **2026-11-19, Asia/Taipei**. The application source version is **0.4.0**.
Library publication, build-only candidate acceptance, formal tagged gates,
GitHub Release publication and device installation are recorded separately below. The [experience report](experience-release-2026-11-18.md)
retains source identities, raw evidence and failed attempts.

## 0.4.0 release

Version 0.4.0 integrates PR #11's experimental ZUI/1 Linux host and fixes
printable numeric-keypad input under Kitty keyboard disambiguation.
The source dependencies require zinput 0.1.2, ztty-ui 0.1.0 and zterm 0.4.0.
Publish the two leaf libraries before zterm, then publish the ztty application.

The tagged application release retains Verify and all six native architecture
gates. Both Linux architectures also run the native UI interaction suite.
The input suite checks 70 byte vectors and 14 zoom cases, including normal
numeric input, explicit application-keypad mode and requested report-all events.
The source-bound results and publication readback accompany the release report.

## 0.3.1 release gate

Version 0.3.1 uses a new immutable `v0.3.1` tag. Publication requires a fresh
Verify job and all six native architecture jobs on that exact tagged source.
Use the [official Releases page](https://github.com/LIghtJUNction/ztty/releases)
for published downloads; a source version or a passing build-only candidate
does not itself establish publication or installation.

The complete candidate at `90ec70fe8359abc02aed1b1d3ced96fd18dd921c` passed
[run 44365691692](https://github.com/LIghtJUNction/ztty/actions/runs/44365691692),
attempt 1, with `build_only=true`, `candidate_platform=all` and both Windows
diagnostic flags false. All six native architecture jobs passed. Linux jobs
used a fresh isolated Rust 1.100.0 installation, avoiding the preinstalled
component conflict observed in run 44362817474.

The Windows workspace harness now recovers recognized sharing conflicts only
when reading its own `state.json` or `keyboard.bin`, for at most one second
from the first error. It records each recovery and propagates permanent or
unrelated errors. This follows the exact failing file-open traceback from
[run 44363824941](https://github.com/LIghtJUNction/ztty/actions/runs/44363824941),
before the next reset chord was sent. It retains the existing workspace, font,
PTY, grid and pixel requirements. Existing UI wait parameters are unchanged;
the bounded file read can extend a cooperative predicate call past its outer
deadline, so it is not a claim of unchanged strict wall-clock duration.

The normal Windows candidate
[`2f2fae44f8e4697759b6061b1f99451f2e0dc3c8`](https://github.com/LIghtJUNction/ztty/commit/2f2fae44f8e4697759b6061b1f99451f2e0dc3c8)
passed [run 44349551432](https://github.com/LIghtJUNction/ztty/actions/runs/44349551432)
with `build_only=true` and `candidate_platform=windows`.

| Architecture | Normal candidate result | Scope |
| --- | --- | --- |
| Windows x64 | Eight fresh processes, each with 31 base checks, 10 warm rapid resizes and 10 back-to-back resize pairs; 19 workspace groups passed | Runtime tracing, painting diagnostics and harness NC-policy diagnostics disabled; final NC readback disabled; owned processes, Job and desktop cleanup passed |
| Windows ARM64 | The same complete eight-process and workspace gate passed natively | The same normal-mode flags, policy readback and cleanup requirements |

The application requests `DWMNCRP_DISABLED` before first showing its retained
GDI window. The harness does not set this policy in a normal run; it reads back
the final value only after the original base and repeated-resize checks pass.
The full-window captures show a standard Win32 frame with a classic light-blue
title bar and complete minimize/maximize/close controls in the runner theme.
The client image is intact. This presentation uses system nonclient drawing
rather than the modern dark DWM frame. If the policy request fails, the
application prints a diagnostic and continues with the available window frame.
The passing candidate does not establish the exact internal writer responsible
for earlier stale-edge frames, nor replace the new tag's all-platform gate.

Version 0.3.1 also contains the merged DEC 1047, G0/G1 cursor-state, height-only
pending-wrap, Linux shifted-punctuation binding and MCP mutation identity-guard
corrections. The [changelog](../CHANGELOG.md#031--2026-11-19-asiataipei) separates
these changes from features carried over from the unreleased 0.3.0 work.

## Local device candidate

A Linux candidate from `19274145e37a591d800f638fe21c2717730b6431` was installed
through pacman as `ztty 0.3.0-1`, independently of GitHub Release publication.
`/usr/bin/ztty` has SHA-256
`57a27b7af8464643cd90889e4930c79e8b97691f2b5d231c28e05830cef7d85c`;
the system executable, user entry and wrapper report `ztty 0.3.0`.
The user entry points to `/usr/bin/ztty`, custom launcher/desktop hashes were
unchanged, package integrity recorded nine files with zero changes, and the
five pre-existing processes retained their PIDs and start ticks after the
executable was replaced. The installed-binary input regression exited 0 with
36 input vectors in 10 groups: 34 used XTEST, while two extended-button cases
used XSendEvent. These are private-Xvfb input checks, not a physical-device
matrix. [Input scope and results](followup-validation-2026-11-18.zh-CN.md#固定源码验证).
[Installed readback](measurements/experience-release-2026-11-18/device-candidate-19274145/installed-readback.json)
and [source-bound scope](measurements/experience-release-2026-11-18/device-candidate-19274145/index.json).

This local candidate is different source from the failed `v0.3.0` tag and is
not an official `0.3.1` installation. Updating an executable also does not
restart an existing broker; a running process can retain its old loaded inode.

## Formal v0.3.0 attempt

[Run 44309412482](https://github.com/LIghtJUNction/ztty/actions/runs/44309412482)
ran from `refs/tags/v0.3.0` with `build_only=false`, `candidate_platform=all`
and attempt 1. The annotated tag object `9317806ac9b098c772f1a05514d222942268d2d5`
resolves to source `9b5497dac5402603ba0f633152fa4b07e6c11c9e`.
[Read-back status](measurements/experience-release-2026-11-18/formal-v0.3.0/github-status.json).

| Formal gate | Result at the tagged source |
| --- | --- |
| Verify | Passed: 271 ordinary Rust tests, 6 distinct doctests, 8 separate SessionCore checks, 17 installer checks, 11 Bash checks, MCP suites and 12 GUI smoke groups; strict Clippy, release build and no_std checks passed |
| Linux x86_64 and ARM64 | Each passed six GUI suites / 67 groups / 27 PNG captures, with clean source before/after, packaged-binary identity and owned-resource cleanup verified |
| macOS Intel and Apple Silicon | Each passed 8 spawn checks and 24 native checks across 13 stages / 10 PNG captures; owned processes cleaned up; ad-hoc signing verified, not notarized |
| Windows x64 | Passed 31 base checks and 19 workspace groups on the original untraced rapid path; owned-process, Job and desktop cleanup passed |
| Windows ARM64 | Failed `resize_restores_visible_image` after 23 of 31 base checks; workspace checks were not reached; cleanup passed |
| Publish | Skipped because the required native gate failed |

The ARM gate restored a 180×190 client area, but its sample at `(175, 28)`
remained `[20,20,20]` after the 10-second pixel wait instead of the required
red `[255,0,0]`. Its original run used `runtime_trace_enabled=false` and
`resize_path=rapid`. Two separate processes subsequently passed all 31 base
checks with tracing enabled, one rapid and one staged. Those runs are diagnostic
only: they leave the original formal result failed and do not establish the
cause of the untraced failure.
[Original failure, screenshot and diagnostic records](measurements/experience-release-2026-11-18/formal-v0.3.0/windows-aarch64-summary.json).

At **2026-11-18 21:11:00 Asia/Taipei (13:11:00 UTC)**, the GitHub Release-by-tag API returned **404**.
There were no official 0.3.0 Release assets to read back or install at that
formal attempt. The later local candidate installation is recorded separately
above. The existing `v0.3.0` tag remains
fixed. After a verified repair, publication must use a new version tag and pass
its own complete gates. Candidate successes below remain historical evidence;
they cannot override this failed formal attempt.
[Compact platform evidence and publication status](measurements/experience-release-2026-11-18/formal-v0.3.0/index.json).

## Development-machine experience checks

The latest Linux view change adds a temporary **Back to live ↓** control while
browsing history, with a new-output indicator and preserved observing ownership.
It leaves the full terminal grid available and avoids open search/palette rows.
Mouse-wheel and Shift+PageUp/PageDown scrolling remain available; see
[daily behavior and limits](daily-workflow.md#returning-to-live-output).
The new device run at harness source `aef55d9` passed all 18 daily GUI groups
using the binary built from `3730b28` (SHA-256
`2a50377a0aea0e53498f0e76c71d969f8dbe7f2585e15b24bc2530063ad4fada`).
It checks real mouse gestures, history anchoring and read-only ownership. The
earlier six-suite Linux candidate below predates this view change.

The subsequent development-machine `ZTTY_BARE_METAL=1 make check` at fixed
source `8a2f4bee22324685f2193e91a16744dda21d0df9` exited 0 after 271 ordinary
Rust tests, 6 doctests and 8 separate SessionCore PTY checks. It also passed
strict workspace Clippy, the release build, production dependency/symbol audits,
17 installer checks, 11 Bash checks, the MCP suites and the portable-library
`x86_64-unknown-none` check. Darwin native tests were explicitly skipped.
[Source and exit status](measurements/experience-release-2026-11-18/scroll-prompt/full-check-8a.json)
and the [full log](measurements/experience-release-2026-11-18/scroll-prompt/full-check-8a.log)
retain the dependency future-compatibility warning. This local gate and the
18-group GUI run have different source identities from the native candidates.

## Native candidate evidence by source

[Run 44293476047](https://github.com/LIghtJUNction/ztty/actions/runs/44293476047)
tested source `a045871a76ff51c1acd29881ff280869e093efbb` with
`build_only=true` and `candidate_platform=macos`.

| Architecture | Candidate result | Evidence and limits |
| --- | --- | --- |
| Apple Silicon | Accepted | 8 threaded-spawn checks and 24 native checks passed; 13 gate stages, 10 PNG captures and an empty owned-process list; native packaging succeeded |
| Intel | Failed before window acceptance | Native compilation succeeded, then the threaded-spawn probe reported `child did not exit`; native window and packaging steps were skipped |

The Apple Silicon captures exercise Sixel colors, transparency, erasure,
cancellation/recovery, window resize and minimize/restore through WindowServer
and ScreenCaptureKit. IME/keyboard input and physical display latency were not
measured. The Intel failure remains distinct from this ARM result.

[Retry 44295578969](https://github.com/LIghtJUNction/ztty/actions/runs/44295578969)
tested source `bfd2732e238b3864e95e721e873f6c9498507bec`, which drains PTY
output while checking child exit. Both native build manifests failed:

| Architecture | Retry result | Evidence and limits |
| --- | --- | --- |
| Apple Silicon | Failed before window acceptance | Metadata and nine concurrent probes reported drained output and child exit; a later `waitpid(-1, WNOHANG)` check returned `0` instead of `-1` with `ECHILD`, so the native window gate was skipped |
| Intel | Failed at the first native RGB check | All 8 threaded-spawn checks passed; native red sampled `[255, 0, 3]` against `[255, 0, 0]`, outside the existing tolerance; 12 native checks passed and 1 failed, with no owned processes remaining after failure cleanup |

[Run 44297585141](https://github.com/LIghtJUNction/ztty/actions/runs/44297585141)
tested source `1ac7fb7` with `candidate_platform=all`. Intel passed all 8
threaded-spawn checks and 24 native checks after the sRGB window-backing fix.
Apple Silicon failed the missing-helper test: `posix_spawn` returned `ENOENT`
while a transient kernel child was still visible. The immediate cleanup
requirement was retained. The subsequent fix checks helper execute
access using the caller's effective identity before starting a child; its
[documented limits](darwin-threaded-pty.md) include filesystem changes between
the access check and execution.

[Run 44298970562](https://github.com/LIghtJUNction/ztty/actions/runs/44298970562)
tested source `2ac46e2fd6e6c373752221d5f1bed9923c67fc1a`, again with
`build_only=true` and `candidate_platform=all`.

| Architecture | Candidate result | Evidence and limits |
| --- | --- | --- |
| Apple Silicon | Accepted | 8/8 threaded-spawn checks, 24/24 native checks, 13 stages and 10 PNG captures; application exit 0 and no remaining owned processes |
| Intel | Accepted | The same 8/8 spawn checks, 24/24 native checks, 13 stages and 10 PNG captures; application exit 0 and no remaining owned processes |

Each macOS job restored the same application PID and window after minimizing.
Independent reads of all 20 sampled PNG pixels per architecture reproduced the
reported values with maximum channel error zero. The earlier failed candidates
remain failed; these accepted results belong to source `2ac46e2`.

The same run accepted both Linux architecture jobs with all six GUI suites.
Windows x64 failed the visible image-restoration check after shrinking and
restoring the client. Windows ARM64 passed its initial 30 native checks, then
failed to launch the workspace probe child. Those failed results remain tied
to `2ac46e2`.

### Windows retry, source dc43870

[Run 44300272576](https://github.com/LIghtJUNction/ztty/actions/runs/44300272576)
tested source `dc43870ef2c8e3c54894d0124e47a23445a3c624` with
`build_only=true` and `candidate_platform=windows`, after deferred resize
handling and the probe's real Enter key were added. Both native architecture
jobs failed `resize_restores_visible_image` during rapid shrink/restore.

| Architecture | Completed checks | Failed sample | Cleanup |
| --- | --- | --- | --- |
| x86_64 | 22 of 30 base checks; 12 visible-pixel checks | Expected red `[255, 0, 0]`, observed `[0, 0, 0]` | Application exit 0, empty owned Job, restored input/thread desktop and closed handles |
| ARM64 | 22 of 30 base checks; 12 visible-pixel checks | Expected red `[255, 0, 0]`, observed `[20, 20, 20]` | The same cleanup requirements passed |

Neither job reached the 19-group workspace suite, so this retry supplies no
workspace acceptance result. The archive and retained report hashes were
verified, and the saved PNG samples reproduce the reported failure. Passing
cleanup does not make the native gate pass. Linux and macOS were outside this
Windows-only retry; their earlier results retain their recorded source scope.

### Windows startup and history retries

The next Windows-only candidates retained each original outcome:

| Source and run | Native result on both x64 and ARM64 | Scope |
| --- | --- | --- |
| `8a2f4bee22324685f2193e91a16744dda21d0df9`, [44303751871](https://github.com/LIghtJUNction/ztty/actions/runs/44303751871) | Failed after 7 base checks: the child could not set its console input mode and reported `WinError 6` | The new host stderr capture exposed inherited redirected handles; resize and workspace checks were not reached |
| `e334cf7ed6bb757f092ebabdcae5291c0d54b03a`, [44305368950](https://github.com/LIghtJUNction/ztty/actions/runs/44305368950) | Passed all 31 base checks, including redirected-host startup and original rapid resize; failed pane history after 5 workspace groups | The original rapid path used no runtime tracing or additional resize acknowledgements; both failed workspace runs cleaned up their owned processes |
| `3418b82dcfa1b9323379e36aec0f3926371b0eb6`, [44306719420](https://github.com/LIghtJUNction/ztty/actions/runs/44306719420) | Accepted: 31/31 base checks and 19/19 workspace groups per architecture | Original rapid resize, redirected-host startup, pane history/selection, keyboard routing, tabs, layout, hidden output and child cleanup passed; owned Job and desktop cleanup also passed |

The `e334cf7` production fix explicitly gives ConPTY children console standard
handles when the host's streams are redirected. The native regression redirects
all three host streams, checks all three child console handles and their actual
20×10 terminal size, and requires the child's output to stay out of the host's
captured streams. The ordinary pixel run again inherits its original stdio;
diagnostic capture remains separate.

The `3418b82` fix changes viewport handling only when keyboard encoding produces
no bytes. Releasing Shift after Shift+PageUp and using silent modifiers for pane
focus now preserve history and selection. Both architectures retained the blue
history sample `[0, 0, 255]` and selection `[34, 68, 102]`; real Enter input
produced `0d` and restored the live red/green pane images. The native checks
retain the original pixels and also verify that silent keys send no PTY input.

Both `3418b82` jobs used `build_only=true` and `candidate_platform=windows`.
The 31-check base run used no runtime tracing or staged resize acknowledgements;
the diagnostic retry paths did not run. Its passing rapid-resize result is a
source-bound observation, not proof of a particular cause for the earlier
resize failures. Linux and macOS receive no new acceptance from these runs.

These candidate records supply evidence for their source commits. Formal
publication requires all six native architecture jobs to accept the exact
final tagged source with `candidate_platform=all`.

## Native acceptance gates

The earlier fifth build-only candidate at `7a1d06b` accepted Linux x86_64/ARM64
with all five GUI suites and Windows x64/ARM64 with all 30 functional checks,
19 visible-pixel checks and full process/Job/desktop cleanup. The sixth
candidate at `ef3c2fc` compiled both macOS helpers, then failed the helper's
activation-policy guard after ten PTY/cwd/protocol checks per architecture.
The [historical source-bound record](native-candidates-2026-11-18.md) preserves accepted,
failed and blocked artifacts, verified hashes and exact download links.
The later `2ac46e2` acceptance above resolves both architectures' macOS gate
failures for that candidate. Native Wayland, physical input, native macOS/Windows
shared sessions and the full performance matrix remain incomplete.

The Release workflow requires six native architecture jobs: Linux x86_64/ARM64,
macOS Intel/Apple Silicon, and Windows x86_64/ARM64. A Windows ARM64 PE built
on x86_64 is not accepted as native ARM64 evidence. The Windows gate checks
the operating system, Python process and tested executable architectures.
Its functional and visible-pixel checks include Sixel, working-directory and
owned-window cleanup cases, with PNG captures on success and failure. Each
architecture must execute these checks on its native Windows runner.
The current Windows gate requires eight fresh processes, each completing all
31 base checks, 10 warm rapid resizes and 10 back-to-back resize pairs, followed
by all 19 workspace groups. After the original checks pass, it also requires
disabled NC rendering and captures the full owned window for frame review.
A passing base report cannot hide a repeated-resize or workspace failure.

Both Linux jobs run `tests/linux_native.py` against a clean, pinned checkout
and the freshly built ELF. Its six suites cover workspace restore, daily
workflow, session switching, Sixel pixels, extended input and bounded repaint
on owned Xvfb displays. The gate records binary, test and helper hashes and
checks process cleanup. These checks establish software X11 behavior; native
Wayland, GPU scanout and physical input devices have separate acceptance scopes.
Native Linux also runs real libc working-directory regressions. The macOS
WindowServer gate retains the PTY/protocol checks and evidence-contract tests,
and reports missing Screen Recording/Accessibility as a blocker.
Its Swift helper requires macOS 14 only for testing; the production binary's
minimum system requirements are unchanged.

Linux evidence is retained in a separate `ztty-linux-ARCH.native.tar.gz`, with
per-suite JSON reports, screenshots and logs. Packaging reads every archived
file back before removing the dedicated staging directory. A failed native
suite or failed evidence archive cannot produce an accepted build manifest.
The flat archive name preserves the existing Release file/checksum contract.

```sh
python3 tests/linux_native_test.py -v
python3 tests/linux_native.py --binary target/release/ztty \
  --source-root "$PWD" --source-sha "$(git rev-parse HEAD)" \
  --arch "$(uname -m)" --version 0.3.1 \
  --output artifacts/linux-native.json \
  --evidence-dir artifacts/linux-native
```

The macOS jobs also run the standalone Darwin threaded-spawn suite before
AppKit acceptance. A Linux skip, nonzero exit, missing/ambiguous evidence,
wrong native architecture or failed timeout/cleanup assertion cannot pass
the separate source-bound spawn report or the candidate build manifest.

## Scoped native candidate validation

Build-only dispatches accept `candidate_platform=all|linux|macos|windows`.
The default remains `all`. Selecting one platform runs both of its native
architecture jobs and keeps source, binary, harness and evidence manifests.
Each build manifest records the selected scope. This lets a platform-specific
repair reach its actual OS without repeatedly rebuilding unchanged targets.
Skipped platforms provide no new acceptance evidence.

```sh
gh workflow run release.yml --ref YOUR_BRANCH \
  -f build_only=true -f candidate_platform=macos
```

Publishing rejects a partial scope even with a matching version tag. The
publish job still requires the verification gate, all native platform jobs,
all six artifact manifests and `candidate_platform=all` in each manifest.
The workflow keeps its existing non-cancelling release concurrency group.
Six local tests execute the actual workflow version/scope shell gate against
14 input combinations, including partial publication, branch publication,
wrong tags and unknown scopes; actionlint passes. The isolated tests supply
only the version and Git facts, not a replacement implementation of the guard.
[Validation record](measurements/release-scope-2026-11-18.json).

## Published packages

The official crates.io version API confirmed these releases and their checksums
at **2026-11-19 00:29:27 Asia/Taipei (2026-11-18 16:29:27 UTC)**; none was yanked.
These libraries are published independently of the application release.

| Package | Published version | Publication time, Asia/Taipei | Official version API |
| --- | --- | --- | --- |
| [zinput](https://crates.io/crates/zinput/0.1.1) | 0.1.1 | 2026-11-18 23:58:46 | [Metadata](https://crates.io/api/v1/crates/zinput/0.1.1) |
| [zgraphics](https://crates.io/crates/zgraphics/0.1.1) | 0.1.1 | 2026-11-19 00:01:48 | [Metadata](https://crates.io/api/v1/crates/zgraphics/0.1.1) |
| [zpty](https://crates.io/crates/zpty/0.2.1) | 0.2.1 | 2026-11-19 00:02:05 | [Metadata](https://crates.io/api/v1/crates/zpty/0.2.1) |
| [zterm](https://crates.io/crates/zterm/0.3.0) | 0.3.0 | 2026-11-19 00:03:51 | [Metadata](https://crates.io/api/v1/crates/zterm/0.3.0) |

The current check establishes registry existence, publication metadata and
checksums. The independent consumer results below belong to earlier package
versions; they are not relabeled as tests of these new releases.

```sh
cargo add zvte@0.1.3 --no-default-features
cargo add zinput@0.1.1 --no-default-features
cargo add zgraphics@0.1.1 --no-default-features
cargo add zpty@0.2.1 --no-default-features
cargo add zterm@0.3.0 --no-default-features
```

### Earlier registry and consumer verification

| Package | Version | Embedded source commit | Publication time, Asia/Taipei |
| --- | --- | --- | --- |
| [zinput](https://crates.io/crates/zinput/0.1.0) | 0.1.0 | `9333047c0867756a73d71e29524ddef3d9f21544` | 2026-11-17 21:50:38 |
| [zgraphics](https://crates.io/crates/zgraphics/0.1.0) | 0.1.0 | `4b3829a0754643e0ff080772db9a1c1886c1fa12` | 2026-11-18 04:03:59 |
| [zpty](https://crates.io/crates/zpty/0.2.0) | 0.2.0 | `4b3829a0754643e0ff080772db9a1c1886c1fa12` | 2026-11-18 04:03:23 |
| [zterm](https://crates.io/crates/zterm/0.2.0) | 0.2.0 | `7535aa29f1a2728a78a446ffea6784dcbc2aae92` | 2026-11-18 04:07:01 |

The crates.io API confirms LIghtJUNction as publisher and all four versions as
not yanked. Cargo downloaded each package from the registry; package checksums
match the API and the embedded VCS metadata matches the table. All four
individual consumers and their combined consumer passed. These consumers use
exact registry versions with no workspace path or Git overrides. Their library
code is `no_std`; their test executables use `std` to drive terminal state,
input encoding, graphics placement/deletion, and a real Linux PTY child.

Evidence: [registry metadata](measurements/registry-0.2-2026-11-18.json) and
[independent consumers](measurements/registry-native-graphics-2026-11-18.json).
The previous 0.1 publication record is retained in
[registry.json](measurements/registry.json).

```sh
cargo add zvte@0.1.3 --no-default-features
cargo add zinput@0.1.0 --no-default-features
cargo add zgraphics@0.1.0 --no-default-features
cargo add zpty@0.2.0 --no-default-features
cargo add zterm@0.2.0 --no-default-features
```

## Earlier 0.2.0 application artifacts and native verification

[Release candidate run 44165748395](https://github.com/LIghtJUNction/ztty/actions/runs/44165748395)
checked out the same immutable source commit in each job. All five native build
jobs succeeded. The archives, application manifests, native reports, and build
reports were downloaded and checked against their recorded hashes. The
[artifact record](measurements/native-artifacts-2026-11-18.json) contains exact
file sizes, SHA-256 values, and source identity.

| Platform | Artifact | Verified scope |
| --- | --- | --- |
| Linux x86_64 | `ztty-linux-x86_64.tar.gz` | Native release build and CLI startup; separate development-machine Xvfb and Hyprland acceptance |
| Linux ARM64 | `ztty-linux-aarch64.tar.gz` | Native release build and CLI startup; ARM64 GUI not exercised |
| macOS Apple Silicon | `ztty-macos-aarch64.zip` and `.tar.gz` | Apple SDK build, real AppKit/Darwin PTY, 8 session/protocol checks, application packaging and signature verification |
| macOS Intel | `ztty-macos-x86_64.zip` and `.tar.gz` | The same native build, 8 checks, and package verification |
| Windows x86_64 | `ztty-windows-x86_64.zip` | Native Win32/ConPTY execution and 17 checks, including actual window pixels |
| Windows ARM64 | `ztty-0.2.0-windows-arm64.zip` | Cross-linked PE and complete bundle verified; native ARM64 execution not performed |

Windows ARM64 has a separate [cross-build record](measurements/windows-arm64-cross-2026-11-18.json).
It is not one of the five native jobs. Those earlier macOS probes left pixel
readback and IME/keyboard acceptance open. Windows x86_64 tests used the unpacked
runtime bundle with compiler directories removed from PATH.

The macOS archives contain `ztty.app`, a launchable native application. Packages
are ad-hoc signed and verified; they do not carry a Developer ID signature or
Apple notarization. The Windows bundle contains `ztty.exe` plus the official
Microsoft ConPTY runtime. Keep its files together when extracting it. Runtime
loading first checks the executable's own directory; the PTY library also
supports the Windows inbox API.

The pinned ConPTY package is `Microsoft.Windows.Console.ConPTY 1.25.261109001`.
Its NuGet SHA-256 is:

```text
3d77f535df848e44225c2a8912ac844fecb23f3057732b639f3034b587496f09
```

The packager verifies the download, PE architecture, complete archive membership,
and per-file hashes. The [Windows packaging tool](../tools/package_windows.py)
and [macOS packaging tool](../tools/package_macos.py) describe reproducible inputs.

The report and deliverables are also archived in the
[ztty Google Drive folder](https://drive.google.com/drive/folders/1XwquftPqNH9H6fjScYw6FPpn5QdxZZxa).
GitHub Actions candidate artifacts have limited retention. The archives in this
section contain the earlier 0.2.0 work. With no arguments, `install.sh` downloads
the latest published application release; the AUR package also follows published
releases. To use changes from the current checkout, build its executable and run
`sh install.sh --from-binary ./target/release/ztty`. A local installation is
independent of GitHub Release and AUR publication.

## Local gates and crate publication

Routine development gates run locally. The earlier 0.2.0 record includes 72 library
unit tests, four doctests, strict workspace Clippy, a Linux release build,
production dependency and symbol checks, and a bare-metal check of the portable
libraries. See [the gate log](measurements/local-gates-2026-11-18.txt),
[no_std audit](measurements/no-std-graphics-2026-11-18.txt), and
[protocol acceptance](protocols.md#acceptance-coverage).

```sh
sh tools/check.sh
cargo test --workspace --doc
cargo run -p zpty --example capture
cargo check -p zinput -p zgraphics -p zterm --target x86_64-unknown-none
python3 tests/gui_smoke.py --binary ./target/release/ztty
python3 tests/graphics_smoke.py --binary ./target/release/ztty
python3 tests/hyprland_graphics.py --binary ./target/release/ztty
```

The last command is an opt-in live desktop check and requires a logged-in
Hyprland session. The Xvfb tests create their own display servers. Native macOS
and Windows probes are run on their corresponding operating systems.

Publish libraries from a clean committed checkout. Verify package contents and
builds before upload, in dependency order: `zinput` and `zgraphics` before
`zterm`; `zpty` is independent. Only publish packages whose versions changed.
The 0.2 publication used local `cargo package` verification and source-bound
native gates, then `cargo publish --locked --no-verify` from a clean development
machine checkout to reuse the already completed build checks. Credentials came
from Cargo's existing configuration and were not copied into source or reports.

```sh
cargo package --locked -p zgraphics
cargo publish --locked -p zgraphics
cargo package --locked -p zpty
cargo publish --locked -p zpty
cargo package --locked -p zterm
cargo publish --locked -p zterm
python3 tools/verify_registry.py
```

## Formal application release

The [Release workflow](../.github/workflows/release.yml) supports two operations.
`workflow_dispatch` with `build_only=true` generates candidate artifacts from a
branch and leaves release publishing and AUR untouched. The earlier candidates
used this operation; the failed `v0.3.0` attempt above used a tag push. The source SHA is pinned before any
platform job starts; failed or incomplete probes cannot produce an accepted
build report.

A matching `vX.Y.Z` tag uses the existing Verify publication gate, all six native
build jobs, source and checksum validation, an Arch package installation check,
then GitHub Release publication and AUR updating. Branch publication is rejected,
and the tag is checked again before publishing. Existing assets must match their
recorded source and hashes before the AUR update can proceed.

The Arch installation check runs `/usr/bin/ztty --version` and checks the
installed desktop entry for `Exec=/usr/bin/ztty` and `StartupWMClass=Ztty`.
The reusable desktop template keeps a generic command; packaging supplies the
system installation path.

```sh
gh workflow run release.yml --ref YOUR_BRANCH -f build_only=true -f candidate_platform=all
# For an approved application release whose Cargo version is X.Y.Z:
git tag -a vX.Y.Z -m 'ztty X.Y.Z'
git push origin refs/tags/vX.Y.Z
```

The AUR step retains the existing `AUR_SSH_KEY` secret and pinned host key in
`packaging/aur/known_hosts`. An older tag cannot downgrade the package. This work
did not change the user's desktop configuration or other project jobs.

## 0.4.1 release attachment contract

All six native platform gates, Verify, source-package checks, and full artifact
provenance checks remain required. The 0.4.0 application and native gates passed,
but uploading 941 standalone attachments triggered a GitHub secondary rate limit.
The 0.4.0 tag and registry packages remain immutable; 0.4.1 uses a new source
commit and new official artifacts.

`tools/release_assets.py` retains every original evidence file in a deterministic
ZIP. Only distribution bundles, their checksums, platform build/native reports,
application manifests, and `install.sh` are standalone assets. The complete set
contains 47 uploads, including the source archive, `SHA256SUMS`, evidence ZIP, and
ZIP checksum; a hard maximum of 64 covers the complete publication set.

Readback first compares the downloaded evidence ZIP with the accepted ZIP,
checks its identity, exact member set, lengths, and hashes, and requires every
standalone distribution file to have been downloaded independently. Only then
does it restore archive-only raw evidence without overwriting existing files.
The original platform-ledger and checksum checks still cover every file.

`tools/release_upload.py` confirms release existence through authenticated
GraphQL, creates only an explicitly absent draft, and reads assets by release ID.
It uploads missing draft assets serially, pauses at least one second after each
write, and stops at the first API error. It never replaces an existing asset or
writes to an already published release. A failed draft can be resumed after the
server-requested cooldown; only a complete successful readback permits publication.
See [GitHub API best practices](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api).

Run both packaging and upload contracts before publication:

```sh
python3 -m unittest discover -s tests -p 'release_assets_test.py'
python3 -m unittest discover -s tests -p 'release_upload_test.py'
```
