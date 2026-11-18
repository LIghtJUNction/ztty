# Darwin PTY creation after host threads start

The new Darwin `Pty::spawn_with_helper` path uses `posix_spawn` to enter a fresh executable image before it performs controlling-terminal setup and executes the requested program. The existing unsafe `Pty::spawn` remains available with its original `forkpty` contract. This change does not make that legacy API safe to call after arbitrary host threads start.

## Why a fresh helper is necessary

Apple's `posix_spawn` file actions run before its `POSIX_SPAWN_SETSID` action. Opening the slave in an `addopen` action therefore cannot by itself give the new session a controlling terminal. The fresh helper runs after the new session exists, calls `TIOCSCTTY` on the already opened slave and verifies that its process group owns the foreground terminal. The helper is the same executable as the host; it replaces itself with the final program, so success leaves one child PID and no additional helper process.

The file-action open uses `O_NOCTTY`. Initial terminal dimensions are set in the fresh helper after this open: XNU's PTY driver rejects extended master ioctls before the first slave open initializes the terminal. Both cell and pixel dimensions are carried to the helper. The native regression starts at 80 columns, 24 rows and 640 by 480 pixels, then checks a subsequent 100 by 30 resize.

The application calls `zpty::run_spawn_helper` immediately after decoding the original arguments, before configuration, working-directory changes or AppKit initialization. `current_executable` obtains an absolute kernel-reported path with `proc_pidpath`; it does not trust `argv[0]`, search `PATH`, or reinterpret a relative path after `cwd` changes. Removing or moving the running executable can prevent a later spawn; this API reports that failure rather than selecting another executable.

## Resource and process ownership

The parent first checks that the absolute helper path is executable using `faccessat(AT_FDCWD, helper, X_OK, AT_EACCESS)`. This uses the caller's effective user and group IDs and runs before allocating a PTY or starting a child. A missing helper or one without execute access therefore returns its actual `ENOENT` or `EACCES` without entering `posix_spawn`. The native SDK test verifies the access-check constants and function prototype.

The parent opens the PTY master as nonblocking and close-on-exec, grants and unlocks it, and obtains the slave name into a caller-owned buffer with `TIOCPTYGNAME`. It creates a private error pipe and moves every private descriptor above fd 2 before constructing any action that replaces standard input, output or error. This ordering also applies when the parent entered the operation with any combination of fd 0, 1 and 2 closed.

`POSIX_SPAWN_CLOEXEC_DEFAULT` restricts helper inheritance to the explicit actions: the slave at fd 0, duplicates at fd 1 and 2, and the error writer. Peer spawns cannot inherit each other's private masters or handshakes. Darwin exposes `pipe` followed by `fcntl`, so there is a short interval before the new pipe's close-on-exec flags are set. Unrelated foreign `fork` or `exec` code must manage its own inheritance policy; this implementation does not promise a process-wide solution for such code. No process-wide signal handler, signal disposition, working directory or environment is modified.

The spawned process starts with an empty signal mask and default catchable signal dispositions. The helper checks that its PID is both the session ID and process-group ID, obtains the controlling terminal, verifies its foreground process group and configures the initial dimensions. It then passes the complete caller-supplied argument and environment byte strings to `execve`. There is no shell quoting step, UTF-8 conversion or implicit environment inheritance.

Once `posix_spawn` succeeds and supplies a PID, that child belongs exclusively to the operation and then to the returned `Pty`. On helper setup, target-exec or handshake failure, a guard checks and reaps that specific PID; if it remains running, it sends `SIGKILL` only to that PID and waits for it. The caller must not arrange automatic `SIGCHLD` reaping or concurrently reap that child. Normal `Pty` hangup, wait and drop behavior remains unchanged.

## Error handshake and its limits

The error pipe carries fixed eight-byte frames: four magic bytes, followed by a little-endian signed error number. A zero error is the ready frame. A positive error reports an operating-system failure. The writer becomes close-on-exec immediately on entry to the helper, so a successful target exec closes it.

The parent accepts EOF only after a valid ready frame. Missing or partial readiness, invalid framing and early helper termination fail instead of appearing as successful setup. Setup and target-exec failures retain their actual errno. The parent reads at most two frames and waits at most five seconds after `posix_spawn` returns; a nonresponsive helper is then cleaned up and reaped. The deadline also applies if reads or polls are repeatedly interrupted. Kernel process creation and completion of a kill/reap operation are not hard-real-time bounded.

Ready followed by CLOEXEC EOF is an exec-handoff protocol, not a proof that the target subsequently ran user code. An external fatal signal in the very small interval between the ready write and `execve` can also close the pipe. The returned child's ordinary wait status remains the authority on later process termination.

The helper access check is not an atomic check-and-exec operation. The path or its permissions can change between the check and `posix_spawn`; an executable file can also have an invalid format or name a missing script interpreter. These cases can pass `X_OK` and still fail in `posix_spawn`. The original operating-system error is returned, and the library does not claim to validate the helper's file format or interpreter.

A failure returned directly by Darwin's `posix_spawn` has a different ownership boundary: no child PID is supplied to the caller. [XNU's implementation](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_exec.c) copies out the PID only on success and internally disposes of a failed spawn. Its internal process can remain visible to a process-wide child query during that disposal. The library neither guesses that PID nor calls `waitpid(-1)` in production, because doing so could reap another component's child.

This boundary was observed in the [ARM candidate at source `1ac7fb7`](https://github.com/LIghtJUNction/ztty/actions/runs/44297585141/job/134778657983): a missing helper returned `ENOENT`, followed by a process-wide nonblocking wait reporting a still-present child. The effective-identity preflight removes that known path before process creation. The immediate child-free assertions remain strict for the covered missing/non-executable helper fixtures and failures after a successful PID handover. They are not a general promise about every kernel-side spawn error or a concurrent path change.

## Native acceptance

The standalone `darwin_spawn` executable test dispatches the helper before starting its own workers. It checks the following against real macOS system calls:

- The SDK's spawn constants, opaque pointer ABI, scalar widths, descriptor flags, ioctl values, structure layout and function prototypes.
- Creating a PTY while two allocator worker threads are active, including exact arguments, non-UTF-8 argument and environment bytes, cwd, input, initial pixel dimensions and resize.
- The child session and process-group IDs, foreground group on all three standard descriptors and an independently opened `/dev/tty`.
- Nine concurrent PTY creations while an unrelated non-CLOEXEC sentinel is open, with no private descriptor surviving target exec.
- Missing and non-executable helper preflight failures, plus target `ENOENT` and `EACCES` failures after PID handover, with the original errno, no descriptor leak and immediate child-free assertions.
- Helper exit, fatal termination or partial readiness before the ready frame, and a stalled helper reaching the five-second timeout.
- Normal hangup and child reaping.
- All seven nonempty combinations of initially closed standard descriptors, each in a dedicated subprocess that cannot alter the test controller's standard input or output.

Run on each native macOS architecture with:

```sh
CARGO_BUILD_JOBS=1 cargo test --locked -p zpty --test darwin_spawn
```

The test invokes the installed Apple SDK's C compiler for the ABI assertions. A Linux run prints an explicit native-platform skip. Cross-target Rust checks can detect type and configuration errors but cannot validate kernel PTY semantics, descriptor inheritance, error behavior or the GUI. The native results below establish acceptance only for their recorded source and test scope.

## Pinned primary sources

The design was checked against Apple's source at the following immutable revisions. Local copies and a SHA-256 inventory are retained in the development evidence directory; production uses the system APIs and does not vendor these sources.

| Question | Apple primary source |
| --- | --- |
| Opaque spawn objects and exact public prototypes | [XNU spawn.h](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/libsyscall/wrappers/spawn/spawn.h) |
| Spawn flags | [XNU bsd/sys/spawn.h](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/spawn.h) |
| File actions before SETSID, explicit descriptor inheritance | [XNU kern_exec.c](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_exec.c) |
| TIOCSCTTY session-leader and foreground-group rules | [XNU tty.c](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/tty.c) |
| First slave-open initialization and master ioctl restrictions | [XNU tty_dev.c](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/tty_dev.c) |
| Descriptor flags and ioctl definitions | [XNU fcntl.h](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/fcntl.h), [ttycom.h](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/ttycom.h) |
| Effective-identity execute-access check and prototype | [XNU fcntl.h](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/fcntl.h), [unistd.h](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/unistd.h) |
| PID handover only on successful spawn and internal failed-spawn disposal | [XNU kern_exec.c](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_exec.c) |
| Master open, grant, unlock and slave-name operations | [Libc grantpt.c](https://github.com/apple-oss-distributions/Libc/blob/71bbe350ab79eef58113991d817ccc6165061a64/stdlib/grantpt.c) |
| Existing controlling-terminal setup order | [Libc login_tty.c](https://github.com/apple-oss-distributions/Libc/blob/71bbe350ab79eef58113991d817ccc6165061a64/util/login_tty.c), [pty.c](https://github.com/apple-oss-distributions/Libc/blob/71bbe350ab79eef58113991d817ccc6165061a64/util/pty.c) |
| Kernel-reported executable path and its size limit | [XNU libproc.c](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/libsyscall/wrappers/libproc/libproc.c), [proc_info.h](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/proc_info.h) |

### Initial compiler and Linux validation

The initial implementation was checked on the Linux development host with the inherited serial Rust wrapper and `CARGO_BUILD_JOBS=1`. These historical compiler and Linux results retain their original scope; they are not native Darwin execution. The detailed command records and stdout/stderr remain in `artifacts/darwin-validation/` in the implementation worktree.

| Gate | Result and scope |
| --- | --- |
| `cargo fmt --all -- --check` | Passed. |
| `cargo check --offline --locked -p zpty --all-targets --target aarch64-apple-darwin` and `x86_64-apple-darwin` | Both passed, including type checking the native test executable. An initial test-only `write` pointer-type warning was corrected to the C `void *` signature before the strict gates. |
| `cargo clippy --offline --locked -p zpty --all-targets --target TARGET -- -D warnings` | Both Darwin targets passed without warnings after that correction. |
| `cargo check --offline --locked --bin ztty --target TARGET` | Both Darwin targets passed with the early helper entry and frontend call. |
| `cargo clippy --offline --locked --bin ztty --target TARGET -- -D warnings` | Both Darwin targets passed without warnings. |
| `cargo run --offline --locked -p zpty --example capture` | Real Linux PTY round trip, 100 by 30 resize and the example's child-exit checks passed. |
| `cargo check --offline --locked -p zpty --all-targets --target x86_64-pc-windows-gnu` | Passed; this is a build check, not native Windows execution. |
| `cargo clippy --offline --locked -p zpty --all-targets -- -D warnings` | Linux passed. |
| `cargo test --offline --locked -p zpty` | Exited successfully on Linux, with zero unit tests and the explicit `SKIP darwin_spawn: requires native macOS` message. No Darwin runtime case was counted as passed. |

No native runtime success or throughput improvement is inferred from these compiler results.

### Native candidate evidence, 18 November 2026

Both macOS jobs in [candidate run `44298970562`](https://github.com/LIghtJUNction/ztty/actions/runs/44298970562) passed at source `2ac46e2fd6e6c373752221d5f1bed9923c67fc1a`, version `0.3.0`. The run used `build_only=true` and `candidate_platform=all`. The following results establish macOS candidate acceptance for that source; they do not publish a release or establish acceptance for another platform.

| Native job | SDK and PTY suite | AppKit and WindowServer suite | Owned-process cleanup |
| --- | --- | --- | --- |
| [ARM64, macOS 15.8.2](https://github.com/LIghtJUNction/ztty/actions/runs/44298970562/job/134783563881) | 8/8 groups; 2,033 allocator-worker iterations; measured stalled-helper timeout 5.004171417 s; suite 6.230032041 s | 24/24 checks, 13 stages, 10 actual PNGs; restored the same PID 1631 / window 23 | Application exit 0; no remaining owned processes |
| [Intel, macOS 15.7.9](https://github.com/LIghtJUNction/ztty/actions/runs/44298970562/job/134783563929) | 8/8 groups; 4,714 allocator-worker iterations; measured stalled-helper timeout 5.004044989 s; suite 8.041714715 s | 24/24 checks, 13 stages, 10 actual PNGs; restored the same PID 2437 / window 26 | Application exit 0; no remaining owned processes |

Each SDK/PTY result includes the effective-identity helper preflight and both missing/non-executable helper and target regressions. Nine concurrent probes still require `DONE`, PTY EOF and a successful exit within the existing two-second exit phase; the five-second stalled-helper limit and immediate child-free assertions remain unchanged. These results exercise the defined ownership contract without claiming that preflight makes every possible kernel-side error synchronous.

Each window suite checked real ScreenCaptureKit captures, including resize, clipping, minimize/restore and subsequent PTY recovery. The three RGB samples were exactly `[255, 0, 0]`, `[0, 255, 0]` and `[0, 0, 255]` on both architectures. An independent read of the saved PNGs reproduced all 20 sampled pixels per architecture with maximum channel error zero; the gate still allows at most one channel value of error. IME/keyboard behavior and physical-display latency remain outside this window suite.

The original reports and screenshots are in the run's [ARM artifact `14424793353`](https://github.com/LIghtJUNction/ztty/actions/runs/44298970562/artifacts/14424793353) and [Intel artifact `14424083947`](https://github.com/LIghtJUNction/ztty/actions/runs/44298970562/artifacts/14424083947). Their ZIP SHA-256 digests are `79eaf1c8a201b260d1c086b91395f6733ab1d46cc08d29bb980bb6dcdf9a72a0` and `f441fcf80f815d3ff84fb5bbc8bc0f4f50febbd1b6cf08238269c2b58ac6a32a` respectively. The audit checked these digests, retained-file build hashes and the four Darwin source-file hashes against the recorded commit. Compact local evidence, per-architecture summaries and PNG sample records are retained under `artifacts/release-audit/all-candidate-44298970562/`.

The package manifests report verified ad-hoc signatures and no notarization. A formal release must rerun its required verification and all six native platform jobs at the final release source; this candidate evidence does not replace that gate.

## Integration and release gate

The shared-layout branch integrates the production implementation at
`6566943a188813fc1c7fcabaac009e63025951d4`. Its Linux full gate passes
202 unit/integration tests, six doctests and eight single-threaded real-PTY
checks, with strict Clippy, release/no_std, MCP and the explicit bare-metal
component gate. The existing zune-png warning remains. The Linux executable
is byte-identical to the previously verified shared-layout executable; a
Darwin test skipped on Linux is explicitly excluded from native acceptance.

The macOS candidate now runs the standalone SDK/PTY suite before its AppKit
pixel suite. It preserves stdout/stderr and source hashes, rejects skipped or
ambiguous results, and requires the actual native architecture, all eight
check groups, active worker iterations and the measured handshake timeout.
Its outcome participates in the build manifest acceptance decision. Six
local gate contracts exercise 16 success/failure inputs against the actual
workflow code, including false/ambiguous evidence, wrong architecture,
non-native hosts and partial timeout output. These contracts do not replace
execution on macOS.

[Integration record](measurements/darwin-integrated-2026-11-18.json),
[full Linux log](measurements/darwin-integrated-2026-11-18.log), and
[gate contracts](measurements/darwin-spawn-gate-contracts-2026-11-18.log)
retain the scope. The original source bundle and complete cross-target logs
are available in the [review archive](https://drive.google.com/file/d/10QSHeEYrKgWOThy3geCMBvyPBrcJPrCb/view?usp=drivesdk).
