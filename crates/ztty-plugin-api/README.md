# ztty-plugin-api

A `no_std` interface for independently built ztty text extensions. The crate defines the text transformation contract shared by the terminal host and a native plugin.

## Interface

`TextAction::transform` reads a UTF-8 string, writes the result into the caller's output buffer, and returns the number of bytes written. `MAX_TEXT_BYTES` is 1 MiB: ztty uses this limit for both input and output.

| Contract | Value |
| --- | --- |
| Interface identifier | `dev.project-z.ztty.text` |
| Interface version | `1` |
| Input | Borrowed UTF-8 text |
| Output | Caller-provided byte buffer and a returned byte length |
| ABI dependency | `zplugin = "=0.3.0"` |

A transformation must write only within the supplied buffer and return `zplugin::Error::BufferTooSmall` when it cannot fit the result. The host checks the returned length and UTF-8 before accepting the transformed text.

## Use

Add the shared contract to the plugin package:

```toml
[dependencies]
ztty-plugin-api = "0.1.0"
zplugin = { version = "=0.3.0", default-features = false, features = ["macros"] }
```

Implement `TextAction` and use zplugin's export mechanism to expose the interface. The host can provide selected text or the visible terminal text; the host's action configuration determines what happens to the returned text.

Native implementations run as trusted native code. They must not retain the borrowed input or output references after the call. Change the interface version when changing method signatures so independently built hosts and plugins can detect an incompatible contract.

See the [host setup and complete plugin example](https://github.com/LIghtJUNction/ztty/blob/main/docs/text-plugins.zh-CN.md) and the [interface source](https://github.com/LIghtJUNction/ztty/blob/main/crates/ztty-plugin-api/src/lib.rs).

## License

MIT. See LICENSE.
