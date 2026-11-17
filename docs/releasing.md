# Releasing Project Z terminal packages

## Current publication record

The source implementation is committed as
[`9333047`](https://github.com/LIghtJUNction/ztty/commit/9333047c0867756a73d71e29524ddef3d9f21544).
The crates.io API confirmed the following records on 2026-11-17:

| Package | Version | Registry state |
| --- | --- | --- |
| [zinput](https://crates.io/crates/zinput/0.1.0) | 0.1.0 | Published by LIghtJUNction; not yanked |
| [zpty](https://crates.io/crates/zpty/0.1.0) | 0.1.0 | Published by LIghtJUNction; not yanked |
| zterm | 0.1.0 | Source complete; publication not yet confirmed; version API returned 404 |

`zterm` packaging verification was started on the developer machine. The
development connection became unavailable before a publication result could be
observed. Check the registry and the existing job before retrying publication.
The first two packages have completed both package verification and publication.
An independent local `cargo publish --dry-run --locked -p zterm` subsequently
passed with registry `zinput`; its [complete log](measurements/zterm-package-dry-run-2026-11-18.log)
shows that upload was explicitly aborted by dry-run mode. It did not publish
the package.

The published `zinput` and `zpty` packages also passed individual and combined
consumer builds outside this workspace, including a `no_std` library check and
real PTY execution. Their registry checksums and embedded VCS commit match the
release source. See the [registry readback](measurements/registry.json) and
[independent-consumer record](measurements/registry-consumer-2026-11-18.json).

```sh
python3 tools/verify_registry.py --packages zinput zpty
# After zterm is available in the registry:
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
