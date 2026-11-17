# zpty

A small Linux pseudo-terminal owner for Rust programs that do not link the Rust standard library. The library uses `core` and `alloc`, has no Cargo dependencies, and calls the system C runtime and `libutil` through a narrow ABI.

`zpty` provides a nonblocking PTY master, an explicitly owned child process, window resizing, partial reads and writes, and child status collection. It leaves polling, terminal emulation, rendering, argument parsing, PATH lookup, and environment policy to its caller.

```rust
use zpty::{Pty, Size};

// This example must run before other process threads are started. Read the
// complete unsafe contract in Pty::spawn before embedding it in a host.
let mut pty = unsafe {
    Pty::spawn(
        c"/bin/sh",
        &[c"sh", c"-c", c"printf hello"],
        &[c"PATH=/usr/bin:/bin", c"TERM=xterm-256color"],
        Size::new(80, 24),
        &[],
    )
}.unwrap();
pty.resize(Size::new(100, 30)).unwrap();
// Register pty.fd() in your poll/epoll loop. read/write return structured
// errors; is_would_block() means wait for readiness before retrying.
```

## Process and memory contract

The caller supplies a resolved executable path, argv including argv[0], and the complete environment. Argument pointer arrays are allocated before forking. No environment mutation, PATH lookup, or Rust allocation occurs in the post-fork child. After `forkpty` returns, that branch uses only `close`, `execve`, `write`, and `_exit`.

`forkpty` itself is not specified as async-signal-safe, so spawning is an `unsafe` operation with an explicit process-wide contract. A program that has already started threads must establish that contract itself or use a separate process launcher. An execution failure is visible as child status 127.

The master has `O_NONBLOCK` and `FD_CLOEXEC` before it is returned. `write` may accept only a prefix; retain the suffix until writable. A hung-up slave produces a zero-length read, including Linux's PTY `EIO` case. `reap` and `wait` collect only this object's child, never unrelated application processes.

`Drop` closes the master, sends SIGHUP to a live child session, and attempts a nonblocking reap. It does not wait indefinitely or force-kill jobs that ignore hangup. Explicitly call `wait` or `wait_timeout` when joining a child is required.

## Run the example

```sh
cargo run -p zpty --example capture
```

The example uses `std` for its printing and timeout harness. The shipped library remains `#![no_std]`. The initial platform scope is Linux; other operating systems need their own PTY ABI and are not advertised as supported.
