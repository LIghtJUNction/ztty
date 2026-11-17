# Releasing Project Z terminal packages

## Current publication record

All three reusable packages are published on crates.io as version **0.1.0**.
Registry readback and isolated consumers were verified on 2026-11-18
(Asia/Taipei). Each package is published by LIghtJUNction and is not yanked.

| Package | Version | Embedded source commit |
| --- | --- | --- |
| [zinput](https://crates.io/crates/zinput/0.1.0) | 0.1.0 | `9333047c0867756a73d71e29524ddef3d9f21544` |
| [zpty](https://crates.io/crates/zpty/0.1.0) | 0.1.0 | `9333047c0867756a73d71e29524ddef3d9f21544` |
| [zterm](https://crates.io/crates/zterm/0.1.0) | 0.1.0 | `3e2f03297c638f14d9799dcb78a915f634287813` |

The second source commit adds documentation and test tools to the same production
Rust implementation. The crates.io checksums and actual publication timestamps
are recorded in [registry.json](measurements/registry.json).

Each package passed an independent consumer, and all three passed a combined
consumer. The generated manifests use exact crates.io versions, without local
path or git overrides. Checks include a `no_std` library, terminal screen/query
behavior, Kitty input encoding, and a real PTY child process. The published
packages' VCS metadata and checksums were read back from Cargo's downloaded
artifacts. See [the complete consumer record](measurements/registry-consumer-all-2026-11-18.json).

```sh
python3 tools/verify_registry.py
```

## Release procedure

The workspace contains three independently reusable libraries and the graphical
`ztty` application. Package names and versions are checked against crates.io
before publication. Release contents must pass local tests and packaging
verification first.

```sh
sh tools/check.sh
cargo test --workspace --doc
cargo run -p zpty --example capture
python3 tests/gui_smoke.py --binary ./target/release/ztty
```

Publish in dependency order. `zinput` and `zpty` are independent; `zterm` depends
on `zinput`; the application depends on `zterm` and `zpty`.

```sh
cargo publish --locked -p zinput
cargo publish --locked -p zpty
cargo publish --locked -p zterm
```

Run the corresponding `cargo publish --dry-run` command before each actual
publication. Dependent packages must resolve their dependencies from the public
registry, so publish and verify `zinput` before packaging `zterm`. Credentials
come from Cargo's configured credential provider or `CARGO_REGISTRY_TOKEN`;
they are never included in source, commands, or reports.

Record registry readback, package checks, commit IDs, and benchmark scope in the
release report. Rebuild a small consumer using registry versions to verify that
the public packages work independently of workspace path dependencies.

Routine development verification runs locally. Release automation, when added,
must preserve these publication and artifact checks without introducing routine
push-triggered test runs.
