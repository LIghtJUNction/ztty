# Darwin PTY creation after host threads start

The new Darwin `Pty::spawn_with_helper` path uses `posix_spawn` to enter a fresh executable image before it performs controlling-terminal setup and executes the requested program. The existing unsafe `Pty::spawn` remains available with its original `forkpty` contract. This change does not make that legacy API safe to call after arbitrary host threads start.

## Why a fresh helper is necessary

Apple's `posix_spawn` file actions run before its `POSIX_SPAWN_SETSID` action. Opening the slave in an `addopen` action therefore cannot by itself give the new session a controlling terminal. The fresh helper runs after the new session exists, calls `TIOCSCTTY` on the already opened slave and verifies that its process group owns the foreground terminal. The helper is the same executable as the host; it replaces itself with the final program, so success leaves one child PID and no additional helper process.

The file-action open uses `O_NOCTTY`. Initial terminal dimensions are set in the fresh helper after this open: XNU's PTY driver rejects extended master ioctls before the first slave open initializes the terminal. Both cell and pixel dimensions are carried to the helper. The native regression starts at 80 columns, 24 rows and 640 by 480 pixels, then checks a subsequent 100 by 30 resize.

The application calls `zpty::run_spawn_helper` immediately after decoding the original arguments, before configuration, working-directory changes or AppKit initialization. `current_executable` obtains an absolute kernel-reported path with `proc_pidpath`; it does not trust `argv[0]`, search `PATH`, or reinterpret a relative path after `cwd` changes. Removing or moving the running executable can prevent a later spawn; this API reports that failure rather than selecting another executable.

## Resource and process ownership

The parent opens the PTY master as nonblocking and close-on-exec, grants and unlocks it, and obtains the slave name into a caller-owned buffer with `TIOCPTYGNAME`. It creates a private error pipe and moves every private descriptor above fd 2 before constructing any action that replaces standard input, output or error. This ordering also applies when the parent entered the operation with any combination of fd 0, 1 and 2 closed.

`POSIX_SPAWN_CLOEXEC_DEFAULT` restricts helper inheritance to the explicit actions: the slave at fd 0, duplicates at fd 1 and 2, and the error writer. Peer spawns cannot inherit each other's private masters or handshakes. Darwin exposes `pipe` followed by `fcntl`, so there is a short interval before the new pipe's close-on-exec flags are set. Unrelated foreign `fork` or `exec` code must manage its own inheritance policy; this implementation does not promise a process-wide solution for such code. No process-wide signal handler, signal disposition, working directory or environment is modified.

The spawned process starts with an empty signal mask and default catchable signal dispositions. The helper checks that its PID is both the session ID and process-group ID, obtains the controlling terminal, verifies its foreground process group and configures the initial dimensions. It then passes the complete caller-supplied argument and environment byte strings to `execve`. There is no shell quoting step, UTF-8 conversion or implicit environment inheritance.

The child PID belongs exclusively to the returned `Pty`. On spawn-handshake failure, a guard checks and reaps that specific PID; if it remains running, it sends `SIGKILL` only to that PID and waits for it. The caller must not arrange automatic `SIGCHLD` reaping or concurrently reap that child. Normal `Pty` hangup, wait and drop behavior remains unchanged.

## Error handshake and its limits

The error pipe carries fixed eight-byte frames: four magic bytes, followed by a little-endian signed error number. A zero error is the ready frame. A positive error reports an operating-system failure. The writer becomes close-on-exec immediately on entry to the helper, so a successful target exec closes it.

The parent accepts EOF only after a valid ready frame. Missing or partial readiness, invalid framing and early helper termination fail instead of appearing as successful setup. Setup and target-exec failures retain their actual errno. The parent reads at most two frames and waits at most five seconds after `posix_spawn` returns; a nonresponsive helper is then cleaned up and reaped. The deadline also applies if reads or polls are repeatedly interrupted. Kernel process creation and completion of a kill/reap operation are not hard-real-time bounded.

Ready followed by CLOEXEC EOF is an exec-handoff protocol, not a proof that the target subsequently ran user code. An external fatal signal in the very small interval between the ready write and `execve` can also close the pipe. The returned child's ordinary wait status remains the authority on later process termination.

## Native acceptance

The standalone `darwin_spawn` executable test dispatches the helper before starting its own workers. It checks the following against real macOS system calls:

- The SDK's spawn constants, opaque pointer ABI, scalar widths, descriptor flags, ioctl values, structure layout and function prototypes.
- Creating a PTY while two allocator worker threads are active, including exact arguments, non-UTF-8 argument and environment bytes, cwd, input, initial pixel dimensions and resize.
- The child session and process-group IDs, foreground group on all three standard descriptors and an independently opened `/dev/tty`.
- Nine concurrent PTY creations while an unrelated non-CLOEXEC sentinel is open, with no private descriptor surviving target exec.
- Actual `ENOENT` and `EACCES` failures, no descriptor leak and no unreaped child.
- Helper exit, fatal termination or partial readiness before the ready frame, and a stalled helper reaching the five-second timeout.
- Normal hangup and child reaping.
- All seven nonempty combinations of initially closed standard descriptors, each in a dedicated subprocess that cannot alter the test controller's standard input or output.

Run on each native macOS architecture with:

```sh
CARGO_BUILD_JOBS=1 cargo test --locked -p zpty --test darwin_spawn
```

The test invokes the installed Apple SDK's C compiler for the ABI assertions. A Linux run prints an explicit native-platform skip. Cross-target Rust checks can detect type and configuration errors but cannot validate kernel PTY semantics, descriptor inheritance, error behavior or the GUI. Native macOS results must be recorded before marking this work accepted.

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
| Master open, grant, unlock and slave-name operations | [Libc grantpt.c](https://github.com/apple-oss-distributions/Libc/blob/71bbe350ab79eef58113991d817ccc6165061a64/stdlib/grantpt.c) |
| Existing controlling-terminal setup order | [Libc login_tty.c](https://github.com/apple-oss-distributions/Libc/blob/71bbe350ab79eef58113991d817ccc6165061a64/util/login_tty.c), [pty.c](https://github.com/apple-oss-distributions/Libc/blob/71bbe350ab79eef58113991d817ccc6165061a64/util/pty.c) |
| Kernel-reported executable path and its size limit | [XNU libproc.c](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/libsyscall/wrappers/libproc/libproc.c), [proc_info.h](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/proc_info.h) |

### Validation record

The Linux development host checked this implementation with the inherited serial Rust wrapper and `CARGO_BUILD_JOBS=1`. The following commands completed successfully; the detailed command records and stdout/stderr remain in `artifacts/darwin-validation/` in the implementation worktree.

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
| Native macOS SDK ABI, worker/concurrency/TTY/descriptor/error/reap suite | **Pending** on each real macOS architecture. |
| Native AppKit application acceptance with this helper | **Pending** on each real macOS architecture. |

No native runtime success or throughput improvement is inferred from these compiler results. The source is ready for the next macOS candidate gate; this document's pending rows must be replaced with its actual evidence before the feature is marked accepted.
