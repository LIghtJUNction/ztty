# ztty-plugin-api

A `no_std` contract shared by the ztty host and independently built native text
extensions.

## Interface

The contract is defined in [src/lib.rs](src/lib.rs):

| Contract | Value |
| --- | --- |
| Interface identifier | `dev.project-z.ztty.text` |
| Interface version | `1` |
| Trait | `TextAction` |
| ABI dependency | `zplugin = "=0.3.1"` |

```rust
fn transform(&mut self, input: &str, output: &mut [u8]) -> Result<u64, zplugin::Error>;
```

The host supplies UTF-8 input and an output buffer. Return the number of bytes
written, or `zplugin::Error::BufferTooSmall` when the result cannot fit. The host
checks the returned length and UTF-8 before accepting the result.

`MAX_TEXT_BYTES` is 1 MiB, the host's input and output limit. Implementations must
respect the actual buffer length and must not retain either borrowed reference
after the call. Native implementations run as trusted code.

Changing a method signature requires a new interface version. This interface
version is separate from the crate's package version.

## Use

For a registry consumer, pin the API and ABI dependency to exact versions:

```toml
[dependencies]
ztty-plugin-api = "=0.1.0"
zplugin = { version = "=0.3.1", default-features = false, features = ["std", "macros"] }
```

The included `examples/dedent-plugin` builds from the local API source. Its
independent Cargo workspace uses these dependencies:

```toml
[dependencies]
ztty-plugin-api = { path = "../../crates/ztty-plugin-api" }
zplugin = { version = "=0.3.1", default-features = false, features = ["std", "macros"] }
```

Implement `TextAction` and export it with `zplugin::export!`. The guest's `std`
feature stays in its own workspace; the shared API remains `no_std`. The host
chooses the selected or visible terminal text and how to use the returned text.
The workspace requires Rust 1.100 or later.

See the [host setup and complete example](https://github.com/LIghtJUNction/ztty/blob/main/docs/text-plugins.zh-CN.md).

## License

[MIT](LICENSE).
