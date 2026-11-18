# Native candidate acceptance — 2026-11-18

The first six-architecture build-only run used source
`fa2f2017b2c54ede7887de4e3ae05d63582efbff`:
[GitHub Actions 44198166920](https://github.com/LIghtJUNction/ztty/actions/runs/44198166920).
It did not publish a Release.

## First native run

| Native runner | Observed result | Scope |
| --- | --- | --- |
| Linux x86_64 | Passed | Workspaces 10, daily workflow 14, Sixel 10, input 27 exact PTY vectors in 8 groups; guarded Xvfb and cleanup |
| Linux ARM64 | Failed | All four GUI suites failed before their first check: `cannot enter session working directory` |
| Windows x86_64 | Passed | 25 checks, including six Sixel pixel groups and actual window PNG captures |
| Windows ARM64 | Failed | Native Python and ztty architecture checks passed; image restore after resize failed before the new Sixel vectors |
| macOS Intel | Passed within existing scope | Eight protocol/session checks; no window pixel or IME acceptance |
| macOS Apple Silicon | Passed within existing scope | Eight protocol/session checks; no window pixel or IME acceptance |

Windows ARM64 reported native machine `0xAA64`; both processes executed
natively. Its sampled point was inside the 180×190 client area, but the
original harness did not record screen placement, occlusion or DC clipping.
The retained failure PNG contains a foreign-looking right strip. This does
not establish a renderer defect or establish a harness-only cause.

These first-run ZIP identities identify candidate builds or failure evidence,
not six accepted production packages.

| Artifact | GitHub artifact ID | ZIP SHA256 |
| --- | ---: | --- |
| Linux x86_64 | 14380631331 | `9438d9b32f281196a70fdb47f41445a5beefb24f7e8c08cb189c0ae7750f0a8c` |
| Linux ARM64 | 14379657965 | `b8111bba6fb165a328990778424d4a92be4236fd7242ad046531df6f56dcd4a1` |
| Windows x86_64 | 14380032435 | `fcd5c1f75bd2c556048ef112ae895173db2efb63dba0f61a75626f20f690142f` |
| Windows ARM64 | 14379792838 | `7cf5078542a079bd88dccffffa9bd93a56976c5ba8740f7391e63bb1da8bab76` |
| macOS Intel | 14380656011 | `05b7ce786ded30f0ba73664327ec0e80ed645e4db14bd6c052fcbfb600cb09c9` |
| macOS Apple Silicon | 14379423318 | `8d653340c2a2ed7220e863c404f137d363e02340b50b8971784868b2464a6c8f` |

## Linux working-directory correction

The session guard opened the old working directory with a literal
`O_DIRECTORY` bit matching x86_64. Linux ARM64 assigns that bit to
`O_DIRECT`, so saving cwd failed before entering the target.
The replacement retains the existing RAII descriptor and uses
`O_PATH | O_CLOEXEC`, whose values agree on both supported Linux ABIs.
This handle also restores a directory that is searchable but not readable.
Save and target-entry failures retain their own immediate errno and have
distinct messages.

See the Linux
[ARM64 UAPI header](https://github.com/torvalds/linux/blob/v6.12/arch/arm64/include/uapi/asm/fcntl.h),
[generic UAPI header](https://github.com/torvalds/linux/blob/v6.12/include/uapi/asm-generic/fcntl.h)
and [open(2)](https://man7.org/linux/man-pages/man2/open.2.html).

Four real libc regressions run as an unprivileged user: ordinary
switch/restore and descriptor flags; an execute-only original directory;
repeated ENOENT/ENOTDIR failures without cwd changes or descriptor leaks; and
actual EMFILE exhaustion before target lookup. The Linux release job now
runs these on each native architecture.

```sh
cargo test --locked --test working_directory -- --test-threads=1
CARGO_BUILD_JOBS=1 ZTTY_BARE_METAL=1 sh tools/check.sh
```

The x86_64 development-machine full gate passed: **174 Rust unit, integration
and documentation tests**, Clippy with warnings denied, release compilation,
Rust `no_std` verification, MCP transport/session/security checks, and the
bare-metal component check. Source identities and original output are in
[the gate record](measurements/arm-candidate-local-gates-2026-11-18.json) and
[log](measurements/arm-candidate-local-gates-2026-11-18.log). An upstream
`zune-png 0.5.2` future-incompatibility warning remains on
`x86_64-unknown-none`.

## Windows sample visibility and failure diagnostics

The revised harness moves only its newly launched, PID-verified test window
inside its monitor work area and makes it topmost. Every asserted
pixel must map through ClientToScreen to that window's own root/PID and pass
PtVisible. Temporary occlusion is polled within the existing ten-second
deadline. Original colors, coordinates and tolerances remain unchanged.

Shrink and restore await exact client dimensions. Each of the three
stages records window/client/screen geometry, DPI, monitor work area, clipping,
sample ownership, observed RGB and a PNG. A failed assertion records the
last valid RGB and final window diagnostics. The screenshot bitmap is cleared
before copying to avoid saving uninitialized pixels when the DC is clipped;
assertions still require actual visible window samples.

Static import/ABI/PNG checks and AST comparison of the original assertions
passed. They do not validate Windows execution. Both Windows architectures,
especially ARM64, must run these unchanged assertions in the next native
candidate before that candidate can be accepted.

## Second native run and remaining corrections

[Run 44201206779](https://github.com/LIghtJUNction/ztty/actions/runs/44201206779)
used `2dc6769b795e407d8fbecc2a305f1045868863a5`. Linux x86_64's four
GUI suites, Windows x86_64's 25 checks and both macOS eight-check protocol
gates passed again. Both ARM64 gates still failed.

Linux ARM64's **release build passed**, but the newly added cwd regression
command first built the debug ztty executable and exposed unresolved
`_Unwind_Resume` imports from precompiled `liballoc`. GNU ld's
`--as-needed` encountered `-lgcc_s` before the Rust archives. The GUI tests
did not run in this second ARM64 job, so the cwd correction was not yet
validated there. A new Linux-only build-script argument places the real
system unwinder after the Rust archives. A separate x86_64 GNU bfd debug build,
four cwd tests, actual ELF imports and the matching system export passed;
[the retained record](measurements/linux-gcc-link-order-2026-11-18.json) does
not substitute that result for ARM64 execution.

Windows ARM64 now retained enough metadata to identify the second failure.
Its 180×190 client area was inside a 1024×720 monitor work area at 96 DPI.
The sample at client (13, 47) read red from ztty's DC, and PtVisible was true,
but WindowFromPoint and the foreground window belonged to PID 5984 rather
than the tested PID 6964. The ownership guard correctly rejected this as
unverified visible output. The original image assertions were not relaxed.
The next harness records window class/root/rect/topmost identities and tries
visible positions for **its own window only**; it never moves, hides or
closes the covering window.

The following downloaded ZIPs and every file in their build manifests were
verified before analysis:

| Failure artifact | ID | ZIP SHA256 |
| --- | ---: | --- |
| Linux ARM64 | 14381332202 | `bc8ff8785fe0752923699ef355103ccc152988ae09418470262b2bbd22c002e0` |
| Windows ARM64 | 14380429063 | `4f21370797aea2fd4fbb852970aa094e2e849b86e84ef3c4e54ff03a847c1e66` |

Their durable copies are in the
[delivery folder](https://drive.google.com/drive/folders/1XwquftPqNH9H6fjScYw6FPpn5QdxZZxa)
as `failed-linux-arm64-44201206779.zip` and
`failed-windows-arm64-44201206779.zip`. Earlier failed-run ZIPs are retained
separately. These filenames deliberately distinguish failure evidence from
accepted installation packages.

## Native startup and macOS pixel additions

The next source candidate adds native `--cwd` with Unicode paths and resolves
relative commands after the directory switch while loading a relative config
before it. Darwin rewrites the child PWD from the physical cwd. The
[startup-directory guide](startup-directory.md) describes the native single-PTY
boundary.

Windows keeps the existing 25 checks and adds five cwd checks. The macOS gate
retains all eight protocol checks and adds owned WindowServer captures through
ScreenCaptureKit, Sixel, resize/minimize recovery and cwd validation. Permission
preflight reports missing Screen Recording or Accessibility as blocked; it does
not edit TCC or substitute internal rendering buffers. The helper's capture is
window-content evidence, not a measurement of physical display latency.

The combined Linux development gate passed 174 Rust tests, Clippy, release,
MCP and no_std checks after these production changes. Windows GNU cross-target
type checking passed; it is not native execution. See the
[local gate log](measurements/native-cwd-link-local-gates-2026-11-18.log) and
[source hashes](measurements/native-cwd-link-local-gates-2026-11-18.json).
Apple SDK compilation and the new macOS/Windows native checks still require
the next actual runner execution.

## Remaining release gates

The linker and visible-location corrections need another source-pinned native
run; re-running either old commit would not test those changes. Native Wayland, real GPU/VRAM, macOS pixels/IME,
physical input matrices and the macOS/Windows shared-session feature set
remain incomplete. Apple Developer ID/notarization and Windows signing
have not been supplied. No full production-release or all-platform
feature-parity claim follows from these candidate gates.
