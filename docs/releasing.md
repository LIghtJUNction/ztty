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

Routine development checks can run locally. Pull requests and release tags also run CI.

## GitHub and AUR

The application release uses the workspace version in `Cargo.toml`. After
merging a version update, push the matching tag to publish it.

```sh
git tag -a vX.Y.Z -m 'ztty X.Y.Z'
git push origin refs/tags/vX.Y.Z
```

The tag workflow runs the same checks as pull request CI, including the Xvfb
GUI suite. It generates a deterministic source archive and SHA256 checksum,
builds and installs the Arch package in a clean container, then publishes the
GitHub Release and pushes the generated PKGBUILD and .SRCINFO to the AUR.
Retry a failed run on the same tag. Existing release assets are checked before
the AUR update, and an older tag cannot downgrade the AUR package.

The release workflow uses the repository secret `AUR_SSH_KEY`. Its public key
must be registered to an account that can maintain `ztty` on the AUR.
`packaging/aur/known_hosts` pins the AUR SSH host key.
