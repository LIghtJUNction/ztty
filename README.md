# ztty

A lean terminal for Linux, written in Rust without the Rust standard library.

ztty aims to exceed Kitty and Ghostty in measured responsiveness and memory efficiency while retaining the terminal behavior that real applications need. The goal is a fast, small, dependable terminal; performance claims must include a reproducible workload and the boundary being measured.

## Design

- `ztty-core`: portable `no_std` screen state, bounded scrollback, protocol handling and input encoding.
- `zvte`: Project Z's allocation-free, batched ANSI/VT parser.
- `ztty`: Linux PTY and X11/XWayland frontend, using the system X11 and Xft libraries through C ABI.
- Normal Rust dependencies must remain `no_std`. Test and benchmark harnesses may use `std`.
- Input handling and screen updates are measured separately from compositor and display latency.

## Development

The repository is being built from its first implementation. Feature support, build instructions and measured results are recorded with the implementation rather than inferred from the design goals.

MIT © LIghtJUNction
