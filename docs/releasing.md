# Releasing ztty and its reusable libraries

Report date: **2026-11-18, Asia/Taipei**. The earlier verified application candidate used
[`7535aa2`](https://github.com/LIghtJUNction/ztty/commit/7535aa29f1a2728a78a446ffea6784dcbc2aae92).
The libraries below are published on crates.io. The application has verified
0.2.0 candidate artifacts from the formal Release workflow; this record does
not claim that a `v0.2.0` GitHub Release or AUR update has been published.

## Roadmap candidate acceptance gates

The roadmap has executed four build-only candidates: fa2f201, 2dc6769,
05782fc and 81c1c06. The fourth accepted both Linux architectures and Windows
x86_64, including all 30 Windows functional checks and cleanup. Windows ARM64
passed ten independent checks and cleanup but was blocked by runner-window
occlusion; both macOS pixel helpers aborted with an initialization assertion.
The [source-bound record](native-candidates-2026-11-18.md) preserves exact
results, verified artifact hashes and durable download links. The next
candidate initializes the macOS helper's WindowServer connection and retains
real exit/signal evidence before attempting JSON decoding.
These are not accepted six-platform production releases.

The newer roadmap branch is not covered by run 44165748395. Its Release
workflow now requires six native architecture jobs: Linux x86_64/ARM64,
macOS Intel/Apple Silicon, and Windows x86_64/ARM64. A Windows ARM64 PE built
on x86_64 is not accepted as native ARM64 evidence. The Windows gate checks
the operating system, Python process and tested executable architectures.
The Windows gate also adds six Sixel pixel/cancellation/recovery groups to the
existing nineteen named checks and saves owned-window PNG captures on success
and failure. These new checks require execution on both native Windows runners;
Linux validation of the test vectors does not establish Win32/ConPTY behavior.

Both Linux jobs run `tests/linux_native.py` against a clean, pinned checkout
and the freshly built ELF. The gate runs workspace restore, daily workflow,
Sixel pixel and extended-input suites on owned Xvfb displays; it records the
binary, test and helper hashes and checks process cleanup. These are software
X11 checks, not native Wayland, GPU or physical input-device acceptance.
The gate's ten failure-path/packaging tests pass locally. Native Linux also
runs four real libc working-directory regressions. The next candidate includes
the corrected system-unwinder link order, native `--cwd`, five additional
Windows cwd checks, and a macOS WindowServer pixel gate. That macOS gate retains
the eight original PTY checks, runs ten evidence-contract tests, and reports
missing Screen Recording/Accessibility as a blocker rather than a success.
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
  --arch "$(uname -m)" --version 0.2.0 \
  --output artifacts/linux-native.json \
  --evidence-dir artifacts/linux-native
```

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

## Application artifacts and native verification

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
It is not one of the five native jobs. The macOS probes explicitly leave pixel
readback and IME/keyboard acceptance open. Windows x86_64 tests use the unpacked
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
GitHub Actions candidate artifacts have limited retention. The current Linux
`install.sh` and AUR package follow published application releases, so building
this source or using these 0.2 candidate archives is required for the new work
until a matching application release is published.

## Local gates and crate publication

Routine development gates run locally. The current result includes 72 library
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

```sh
gh workflow run release.yml --ref native-graphics-0.2 -f build_only=true
# For an approved application release whose Cargo version is X.Y.Z:
git tag -a vX.Y.Z -m 'ztty X.Y.Z'
git push origin refs/tags/vX.Y.Z
```

The AUR step retains the existing `AUR_SSH_KEY` secret and pinned host key in
`packaging/aur/known_hosts`. An older tag cannot downgrade the package. This work
did not change the user's desktop configuration or other project jobs.
