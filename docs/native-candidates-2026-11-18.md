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

## Third native run: Linux ARM64 accepted, harness corrections

[Run 44206278093](https://github.com/LIghtJUNction/ztty/actions/runs/44206278093)
used `05782fc23b23306d61932ad39777407802886548`. It was build-only and
did not publish a Release.

| Runner | Actual result |
| --- | --- |
| Linux x86_64 and ARM64 | Four cwd regressions and all four native GUI suites passed: workspaces 10, daily 14, Sixel 10 and input 27 vectors in eight groups; owned-process cleanup passed |
| Windows x86_64 | All 30 functional checks, including the five new cwd checks, passed; the gate still failed because it attempted to remove the live process's cwd before shutdown |
| Windows ARM64 | Native architecture and four early cwd checks passed; all nine attempted positions remained covered by a different full-screen CoreWindow; the same cleanup error then masked the original visibility failure |
| macOS Intel and ARM64 | Eight original protocol checks, cwd/PWD, relative config, architecture and invalid-cwd checks passed; both permissions were granted; window selection failed before any capture |

Linux ARM64 now verifies both preceding production fixes on the actual
architecture: the portable cwd handle and the real unwinder's link order.
The ARM64 workspace and Sixel PNGs were inspected and show intact text,
padding and image regions. The Windows x86_64 Sixel PNG was also inspected.
These captures establish the stated software/native window scope; they
do not measure hardware scanout, IME or input-to-display latency.

### Windows lifecycle and visibility

The former temporary-directory context exited before the outer process
shutdown. Windows keeps a running process's cwd locked, as documented by
[SetCurrentDirectory](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setcurrentdirectory).
The corrected harness closes and waits for its owned process before removing
the directory. Cleanup errors remain separate from the first failure.
Five contract tests cover real child-before-directory disposal on success and
failure, bounded forced shutdown, cleanup errors and failure before launch.

The ARM64 covering window had class `Windows.UI.Core.CoreWindow`, rectangle
`[0, 0, 1024, 768]`, topmost status and PID 6228; the tested window belonged
to PID 7288. These facts prove occlusion, not the covering process's purpose.
The next gate records only executable basenames and read-only desktop names
or access errors to resolve that remaining question. It does not move,
close, unlock or switch another window or desktop. Invisible samples retain
a nonzero blocked result. Independent cwd and protocol checks run before the
visibility requirement. All 30 existing functional/pixel assertions, their
color tolerance and the ten-second pixel deadline remain unchanged.

### macOS window identity

Both helpers successfully received `SCShareableContent`, then rejected six
windows belonging to the same application PID. The last error was
`expected exactly one owned window ... found 6`; no
`SCScreenshotManager.captureImage` call or PNG followed. This was a selector
failure, not evidence of a permission or pixel-rendering failure.

The child now sends a unique OSC 2 title through the actual PTY. The helper
requires that title, PID, normal window layer and visible state, then pins the
first accepted WindowID. AX resize/minimize/restore separately require the
same PID/title and standard-window role. Zero or multiple matching windows
still fail. Candidate inventories make future failures inspectable. Existing
colors, geometry, capture method and deadlines remain unchanged.

The original ten Python evidence-contract tests pass. The helper's new pure
selection vectors and actual window captures still need the next native
Apple SDK run; they are not counted as completed pixel acceptance.

### Third-run artifacts

GitHub reports the following archive identities. Downloaded Linux ARM64,
both Windows and both macOS ZIPs and their build-manifest members were
checked for source, size and SHA-256 before review.

| Artifact | ID | ZIP SHA256 |
| --- | ---: | --- |
| Linux x86_64 | 14383752166 | `6c224b66fa80631f0848750a9c01bc61b3e2976b9094c14f4729788dfd0a7e2d` |
| Linux ARM64 | 14384301394 | `8e2e8f820726f19e7519ebdffa1fb5ec802735a380cc8645cd15beb75fef4845` |
| Windows x86_64 | 14383244437 | `778280b5cf2b3fc16afe9e8decbe5ba0980d7cd868321e821c8777e6c3928b0b` |
| Windows ARM64 | 14384425989 | `c93197ffcb1badddda79fb07cfe309b5d04f10b4948fd48dbddd075c9a0b8dec` |
| macOS Intel | 14384126865 | `546d00bfb2017be157d14d712f7d192c168ac1069d9b9c5efce2376f2e82375d` |
| macOS ARM64 | 14383398590 | `bb64291c275e18da0122ee019d6cf3f85f4dfdbe25100562edf61d3553356a40` |

[Machine-readable run record](measurements/native-third-run-2026-11-18.json)
and [local harness checks](measurements/native-gate-repair-2026-11-18.json)
retain the source and scope. The accepted Linux ARM64 candidate is also
[available here](https://drive.google.com/file/d/1N_J8iIRLF71ROT_qd5Nh02G_NJe3D_RZ/view);
the [delivery folder](https://drive.google.com/drive/folders/1XwquftPqNH9H6fjScYw6FPpn5QdxZZxa)
retains separately named failed Windows archives.

## Fourth native run: Windows x86_64 accepted

[Run 44209439093](https://github.com/LIghtJUNction/ztty/actions/runs/44209439093)
used `81c1c068c42868d4e1fdfb1e97e9046c4094f361` in build-only mode.
Linux x86_64 and ARM64 passed the four GUI suites and cwd regressions.
Windows x86_64 passed all 30 functional checks, five harness contracts and
owned-process/directory cleanup. The actual Sixel PNG shows the expected
red, green and blue regions; the Linux ARM64 workspace PNG retains both
panes, text and padding. This run did not publish a Release.

Windows ARM64 passed ten independent startup/cwd/protocol checks and cleanup.
Every attempted visible position remained covered by `WWAHost.exe`, PID 9188,
class `Windows.UI.Core.CoreWindow`, topmost rectangle `[0,0,1024,768]`;
ztty belonged to PID 11048. Thread and input desktop names were both
`Default`. The native report preserves `blocked` and exits nonzero;
the build report remains failed. No foreign window was closed or moved.

Both macOS architectures passed twelve protocol/startup checks and helper
preflight, including the pure selection vectors. The first Sixel pixel
probe produced empty stdout and the WindowServer initialization assertion
`CGS_REQUIRE_INIT` with `did_initialize` false. No PNG was produced.
The previous harness attempted JSON decoding before recording return status,
so the helper's actual signal and exact capture stage are unknown.

The helper now initializes `NSApplication.shared` on the main actor, using
the prohibited activation policy. Apple's [shared documentation](https://developer.apple.com/documentation/appkit/nsapplication/shared)
states that it establishes the WindowServer connection.
The [prohibited policy](https://developer.apple.com/documentation/appkit/nsapplication/activationpolicy-swift.enum/prohibited)
prevents helper windows and activation. Process exit/signal, stderr and capture
phase checkpoints are retained even when no JSON is returned. The ten Python
contracts pass, including a real empty-stdout SIGTERM case. This correction
still needs native execution; it does not establish pixel acceptance.

All six ZIP downloads were checked against GitHub's byte counts and SHA-256.
Every build-manifest member was checked against its size/hash and pinned
source before review. The [machine-readable record](measurements/native-fourth-run-2026-11-18.json)
contains exact artifact IDs, hashes and individual Drive links. The accepted
Windows x86_64 and both Linux candidates, plus separately named failed or
blocked evidence, are in the
[delivery folder](https://drive.google.com/drive/folders/1XwquftPqNH9H6fjScYw6FPpn5QdxZZxa).

## Owned Windows CI desktop

The next candidate runs the unchanged Windows native gate on a uniquely named,
temporary desktop owned by the test. This is an explicit CI-only path, limited
to the named Windows runners with a visible WinSta0 station and Default
thread/input desktop. Secure, noninteractive or inaccessible contexts remain
blocked.

The controller creates the worker suspended, passes only its three stdio
handles, and verifies membership in an owned kill-on-close Job before starting
it. It makes the owned desktop visible, then the worker independently verifies
its actual thread/input desktop before running the original 30 checks.
WindowFromPoint, PtVisible, pixel colors and per-pixel deadlines are unchanged.
Foreign windows are not moved, hidden or closed.

The controller monitors input-desktop identity during the run. A change to an
unrelated or secure desktop aborts the test and only its owned Job is cleaned
up. Success additionally requires the worker to exit, its Job to become empty,
the original desktop to be restored, and owned handles/desktops to close.
The worker keeps its 180-second limit; the outer 225-second budget includes
bounded cleanup and restoration. Cleanup failure cannot pass the candidate.

The [source and contract record](measurements/windows-desktop-gate-2026-11-18.json)
and [fourteen passing failure contracts](measurements/windows-desktop-gate-2026-11-18.log)
document local verification. These fixtures do not establish actual native
desktop switching or pixels. The next source-pinned x64 and ARM64 jobs must
supply those results and preserve the independent worker/desktop reports.

This follows the documented [SwitchDesktop](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-switchdesktop),
[explicit handle inheritance](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)
and [Job lifecycle](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
contracts. No security settings or permissions are modified.

## Remaining release gates

The fourth run accepted both Linux architectures and Windows x86_64.
macOS helper initialization still requires a source-pinned native rerun;
Windows ARM64 needs an unobstructed, owned test desktop while preserving
real visible-pixel checks. Native Wayland, GPU/VRAM, macOS pixels/IME,
physical input matrices and macOS/Windows shared sessions remain incomplete.
Apple Developer ID/notarization and Windows signing have not been supplied.
No full production-release or all-platform feature-parity claim follows
from these candidate gates.
