# zpty

Native pseudo-terminal ownership for Rust programs that do not link the Rust standard library. `zpty` uses `core` and `alloc`, has no Cargo dependencies, and exposes bounded, nonblocking reads and writes with explicit child-process ownership.

Version 0.2 adds macOS PTYs and a Windows ConPTY implementation. The caller owns terminal emulation, rendering, executable lookup, environment policy, and its event loop. The operating-system APIs differ, so the constructors and readiness handles make those differences explicit.

| Platform | Session API | Readiness | System boundary |
| --- | --- | --- | --- |
| Linux | `unsafe Pty::spawn` | `Pty::fd()` for poll/epoll | C runtime and `libutil` |
| macOS | `unsafe Pty::spawn` | `Pty::fd()` for poll/kqueue | Darwin C runtime |
| Windows 10 build 17763+ | `Pty::spawn_utf16` | `output_event()` and `process_event()` for native wait sets | ConPTY and Win32 worker threads |

## Linux and macOS

Supply a resolved executable path, argv including argv[0], a complete environment, and descriptors that must be closed in the child. Argument pointer arrays are prepared before the fork. After `forkpty` returns, the child uses only `close`, `execve`, `write`, and `_exit`; it does not allocate Rust memory or search PATH.

```rust,no_run
# #[cfg(unix)] {
use zpty::{Pty, Size};

// Run before other process threads are started. Read the full safety contract
// on Pty::spawn before embedding this operation in a multithreaded host.
let pty = unsafe {
    Pty::spawn(
        c"/bin/sh",
        &[c"sh", c"-c", c"printf hello"],
        &[c"PATH=/usr/bin:/bin", c"TERM=xterm-256color"],
        Size::new(80, 24),
        &[],
    )
}.unwrap();
pty.resize(Size::new(100, 30)).unwrap();
// Register pty.fd() with the host's native event loop.
# }
```

`forkpty` itself is not specified as async-signal-safe. The `unsafe` constructor therefore has a process-wide safety contract: callers must establish that forking is safe for their host, C runtime, allocators, and signal handlers. A host that cannot establish this contract needs a separate process launcher. An `execve` failure appears as child exit code 127.

The returned master has `O_NONBLOCK` and `FD_CLOEXEC`. A closed slave produces a zero-length read; Linux's slave-hangup `EIO` is normalized to EOF. Unix `Drop` closes the master, sends SIGHUP to the child session if still live, and attempts a nonblocking reap. It does not force-kill jobs that ignore hangup or wait indefinitely for them.

## Windows

`spawn_utf16` accepts a resolved executable path, argv including argv[0], an optional Windows environment block, and character dimensions. Argument strings exclude their NUL terminator. The library builds a mutable command line using Windows CRT escaping rules and rejects commands exceeding 32767 UTF-16 units. `None` inherits the environment; a supplied block must be sorted case-insensitively and end with two NUL units.

```rust,no_run
# #[cfg(windows)] {
use zpty::{Pty, Size};

// Replace this with the host's resolved executable path; do not assume every
// Windows installation uses C:\Windows.
let program: Vec<u16> = r"C:\Windows\System32\cmd.exe".encode_utf16().collect();
let option: Vec<u16> = "/D".encode_utf16().collect();
let pty = Pty::spawn_utf16(
    &program,
    &[&program, &option],
    None,
    Size::new(80, 24),
).unwrap();
pty.resize(Size::new(100, 30)).unwrap();
// Wait on pty.output_event() and pty.process_event() in the native event loop.
// Handles are borrowed; callers must not close them.
# }
```

ConPTY requires synchronous pipe endpoints. Two native workers service these pipes so the public `read` and `write` methods do not block the UI on pipe I/O. Each direction has a queue capped at **1 MiB**, plus a 16 KiB worker buffer. Queue storage grows on demand. This bound describes the library's queues, not the memory used by Windows, OpenConsole, or the child application.

The output readiness event is manual-reset. Drain `read` until `is_would_block()` before waiting again. `write` accepts only the prefix that fits; callers retain and retry the suffix. `process_event` is signaled when the directly owned child exits. `reap`, `wait`, and `wait_timeout` query that child's handle and cannot consume an unrelated process's status.

Windows `hangup` closes the pseudoconsole and its attached console processes. The reader keeps draining the final console output during this operation, including when its public output queue was full. Windows `Drop` then cancels outstanding worker I/O, joins the workers, and closes their handles. This teardown can block while Windows closes the console; it has different lifetime semantics from the nonblocking Unix destructor. Microsoft's [pseudoconsole session guide](https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session) explains the underlying pipe-drain requirement.

### Optional official ConPTY redistribution

The library prefers an adjacent `conpty.dll` and `OpenConsole.exe` pair supplied by the application. It obtains the executable's absolute directory and loads the DLL with `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32`; it does not search PATH or the current working directory. If the pair is absent or cannot provide the required exports, the library uses the inbox Windows API. `uses_bundled_conpty()` reports which backend the session selected.

The ztty repository's `tools/package_windows.py` packages the executable with Microsoft's **1.25.261109001** ConPTY redistribution, pinned to an exact package SHA-256, and includes its MIT license and a file-hash manifest. The [official package](https://www.nuget.org/packages/Microsoft.Windows.Console.ConPTY/1.25.261109001) supports Windows build 17763 and later. Microsoft describes the redistribution model in [terminal discussion 17608](https://github.com/microsoft/terminal/discussions/17608).

ConPTY version affects escape-sequence transport. A successfully linked Windows executable alone does not establish Kitty Graphics compatibility, cursor behavior, or native rendering correctness. Run the native acceptance test against the actual packaged pair; no behavior is inferred from an undocumented passthrough flag.

## I/O and validation

On every platform, a successful `write` may accept fewer bytes than supplied. Preserve the unwritten suffix, and use `is_would_block()` to distinguish a temporarily full channel from a permanent error. `wait` deliberately blocks; use `reap` or `wait_timeout` when the caller needs a bound on child-status collection.

```sh
cargo run -p zpty --example capture
```

The example selects `/bin/sh` on Unix and native PowerShell on Windows. It verifies round-trip text, the resized 100-column by 30-row console, successful child exit, and a separately owned process exiting with code 7. Its timeout and printing harness uses `std`; the library remains `#![no_std]`.

The Windows frontend acceptance gate is `python tests/windows_native.py --binary PATH_TO_ZTTY_EXE`. It requires a real Windows desktop and checks pixels read from the native window after real ConPTY traffic. Cross-target type checking, Clippy, PE linking, and archive checks are separate build gates and do not replace this native test. macOS likewise requires an actual macOS run to validate AppKit and Darwin PTY behavior.
