# Dedent text extension

This separately linked `cdylib` removes common leading ASCII spaces from copied
code, while retaining relative indentation, Unicode, and line endings. Tabs are
left intact. The terminal decides the input: a selection, or the visible viewport
when no selection exists. The extension never receives the PTY file descriptor.

The example uses the local `ztty-plugin-api` source and pins `zplugin = "=0.3.1"`.
The API package 0.1.0 is awaiting its first registry release.

From the ztty repository root:

```sh
cargo build --release -p ztty
cargo build --release --manifest-path examples/dedent-plugin/Cargo.toml
plugin_path=$(realpath examples/dedent-plugin/target/release/libztty_dedent_plugin.so)
./target/release/ztty plugin check "$plugin_path"
printf '    first\n      second\n' | ./target/release/ztty plugin run "$plugin_path"
env -u ZTTY_SOCKET ./target/release/ztty --text-plugin "dedent=$plugin_path"
```

In that window, select text, press **Ctrl+Shift+P**, type `dedent`, and press
**Enter** on **Copy using dedent**. The result goes to the clipboard. It is never
automatically pasted into the shell. Escape cancels a running call. Empty or
failed results keep the existing clipboard.

For persistent registration, add an absolute path to the ztty configuration:

```ini
text-plugin = dedent=/absolute/path/libztty_dedent_plugin.so
```

Each invocation creates a fresh worker and interface instance, destroys the
instance, and cooperatively unloads the library before accepting its result.
The guest has its own Cargo workspace so its `std` feature cannot enter the
terminal's `no_std` dependency graph. The shared interface remains `no_std`.

Native extensions execute with the user's privileges and must be trusted. The
Linux worker limits faulty execution to 1 MiB input, 1 MiB output, a 2 second
parent deadline, 1 CPU second, 256 MiB virtual address space, and 64 descriptors.
These are process and protocol limits, not a security sandbox. The current GUI
host is Linux only and requires Linux 5.9 or newer for `close_range`.

See [the host and interface guide](../../docs/text-plugins.zh-CN.md) for the API,
registration limits, and reproducible fault and GUI checks.
