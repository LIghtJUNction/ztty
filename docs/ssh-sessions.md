# SSH sessions

ztty 0.6.0 can open a Linux terminal session over SSH and keep its PTY running
after the connection closes. The remote ztty broker owns the shell, parsed screen
and history. An SSH client is a view of that state. Reattaching a named session
keeps its process and shell variables; its creation command runs once.

This implements persistent SSH sessions in ztty. Both ends use ztty's own
session protocol; it does not connect to a Rex Server or use Rex's wire format.

## Connect and reconnect

Install ztty 0.6.0 or later on the local and remote Linux machines. The remote
executable is found on the SSH user's PATH or at `~/.local/bin/ztty`.

```sh
ztty ssh dev main
ztty ssh ssh://alice@build.example:2222 build
ztty ssh -i ~/.ssh/build_key dev build
ztty ssh -F ~/.ssh/work_config dev build
ztty ssh --remote-bin /opt/ztty/bin/ztty dev build
```

The first command creates `main` if it is absent and otherwise reattaches it.
The default session name is `main`. Names contain 1–64 ASCII letters, digits,
dots, underscores or hyphens. SSH aliases, IPv6 addresses, ports, identity files,
agents and host-key checks use the system OpenSSH client.

Press **Ctrl+]** to detach, including when an application enables Kitty's
enhanced keyboard protocol. Closing the local window also disconnects its view.
Run the same command to reconnect. A session that has finished keeps its final
screen and exit status until explicitly closed.

```sh
ztty ssh --list dev
ztty ssh --close dev build
```

The headless broker survives the SSH connection. A reboot, killing the broker,
or a host policy that kills every process in a logged-out user's account ends
its processes. It does not save running processes across machine restarts.

## Tabs, splits and ownership

In an SSH window, **Ctrl+Shift+T** creates another remote shell on the same host.
**Ctrl+Shift+E/O** splits right/down. The new pane gets an independent generated
name beginning with `pane-`; `ztty ssh --list HOST` lists it for later reattachment.
These actions inherit connection settings, and use the remote default shell.
A previous pane's explicit creation command is not rerun in a new pane.

The CLI can target an existing local ztty view using `--tab LOCAL_SESSION`,
`--split-right LOCAL_SESSION` or `--split-down LOCAL_SESSION`. Both commands must
address the same local broker (`ZTTY_SOCKET` selects it). Native tabs and splits
currently use X11/XWayland. Independent windows also work on native Wayland.

```sh
ztty ssh --observe dev build
ztty ssh --takeover dev build
```

An observer receives frames, discards input and leaves the writer's PTY size
unchanged. A smaller observer sees the top-left portion; a larger one adds blank
space. Attaching a second writer requires `--takeover`. Normal detach releases
control; reconnect can reclaim a dead mux process using its PID and process
start identity. Session identity checks protect every read and mutation from a
closed-and-recreated name.

OpenSSH shares the authenticated connection with `ControlMaster=auto` and a
60-second idle `ControlPersist`. Its control socket lives in an owner-only
runtime directory. Each remote pane still has its own PTY and process.

## Commands and headless use

Arguments after `--` create the session's initial program. They are passed as
literal arguments; use an explicit shell when shell syntax is wanted.

```sh
ztty ssh dev build -- bash -lc 'cargo build; exec bash'
ztty ssh --terminal dev build
```

`--terminal` displays the remote session in the invoking terminal. The same
multiplexer can also be used locally without a desktop:

```sh
ztty mux attach work -- /bin/bash
ztty mux list
ztty mux attach --observe work
ztty mux close work
```

The default headless mux has a separate per-user broker namespace. An explicit
`ZTTY_SOCKET` can isolate a test or choose an existing compatible broker. No
additional network listener or tmux process is started.

## Display and performance

The remote broker parses terminal output and answers device queries. Its view
transfers text cells, colors and attributes, Unicode graphemes, hyperlinks,
cursor state, alternate-screen state and input modes. Reconnect includes up to
2,000 recent history rows, within a 480 KiB frame budget. It projects terminal
text; Kitty/Sixel image data, shell command marks and soft-wrap metadata are not
included in this SSH view.

An unchanged session parks a revision watcher without copying output or walking
the screen. Changed frames are limited to one per 16 milliseconds; only changed
rows cross SSH. The terminal input queue and one pending output frame provide
bounded backpressure. A slow or disconnected viewer cannot accumulate an
unbounded transcript in the broker.

Input acknowledgements wait until the remote PTY has accepted the entire input
chunk. This prevents a large paste followed by **Ctrl+]** from losing its queued
suffix. A PTY that stops accepting input for five seconds produces an explicit
error; the client restores its terminal and releases input control.

## Reproduce the checks

```sh
cargo test --locked --features wayland --test ssh_command --test mux_input --test terminal_frame
cargo build --locked --release --features wayland
python3 tests/ssh_mux.py --binary target/release/ztty --output artifacts/ssh-mux.json
python3 tests/ssh_mux_gui.py --binary target/release/ztty \
  --output artifacts/ssh-mux-gui.json --screenshot artifacts/ssh-mux-gui.png
cargo run --locked --release --example ssh_frame_benchmark
```

The SSH fixture creates temporary keys, a pinned known-host entry and a
localhost-only sshd. It checks separate PTYs over one authentication, connection
loss, reconnect without command reexecution, resize, observer isolation, byte
input and exact exit status. GUI checks use an owned X server. Both fixtures
clean up their own processes and leave existing sessions untouched.
