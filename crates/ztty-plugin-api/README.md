# ztty-plugin-api

A `no_std` interface for independently built ztty text extensions. An extension
transforms selected text or the currently visible terminal text.

## Interface

The contract is defined in [src/lib.rs](src/lib.rs):

- Interface ID: `dev.project-z.ztty.text`
- Interface version: `1`
- Trait: `TextAction`

```rust
fn transform(&mut self, input: &str, output: &mut [u8]) -> Result<u64, zplugin::Error>;
```

The host supplies both buffers. Return the number of output bytes written.
If the output buffer is too small, return `zplugin::Error::BufferTooSmall`
without writing beyond it. ztty checks the returned length and UTF-8 before
accepting the result.

`MAX_TEXT_BYTES` is 1,048,576 bytes: ztty's per-action input and output limit.
Implementations must still respect the actual length of the supplied buffer.
Input and output references must not be retained after the call. Native
implementations are trusted code; the host controls loading and execution.

Changing a method signature requires a new interface version. This interface
version is separate from the crate's package version.

## Dependency and license

The crate uses `zplugin = 0.3.0`, with default features disabled and the
`macros` feature enabled. The workspace requires Rust 1.100 or later.

Licensed under [MIT](LICENSE).
