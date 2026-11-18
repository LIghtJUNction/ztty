# Releasing ztty and its reusable libraries

Report date: **2026-11-18, Asia/Taipei**. The source tree prepares application
**0.3.0**; final-tag verification, publication and device installation remain
pending. The earlier verified 0.2.0 application candidate used
[`7535aa2`](https://github.com/LIghtJUNction/ztty/commit/7535aa29f1a2728a78a446ffea6784dcbc2aae92).
The libraries below are published on crates.io. Their source commits and the
earlier application candidate artifacts remain recorded independently of the
0.3.0 release preparation. The [experience candidate report](experience-release-2026-11-18.md)
pins the Linux runtime, revised GUI harness and native retry evidence separately.

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
  --arch "$(uname -m)" --version 0.3.0 \
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
branch and leaves release publishing and AUR untouched. This is the operation
used for the current native verification. The source SHA is pinned before any
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
