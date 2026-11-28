# ztty CLI through termlens

This isolated, unpublished Cargo workspace tests the **real release ztty
executable** in Linux pseudo-terminals. It covers supported noninteractive
paths without X11, Wayland, a broker, or a shell wrapper:

- `--help`: success, Unicode em dash, complete usage/footer and wrapping at
  40 versus 120 columns (200 rows keep the whole help visible).
- `--version`: exact package version and exit status 0.
- `--geometry 1x24`: exact diagnostic and exit status 1 before UI startup.

The child environment is cleared, so a display or user config cannot make
these checks pass. Each process wait has a five-second deadline; tests wait
for exit and the harness's output drain before examining a single final
screen. No sleeps or golden snapshots are used. These are short-lived CLI
paths on Linux; termlens documents potential output loss for immediately
exiting children, especially on macOS. Do not broaden platform claims without
measuring that behavior.

**The screen grid and wrapping here are termlens's VT rendering of ztty's
stdout/stderr transcript. They do not test ztty's native window, zterm parser,
PTY session management, fonts, graphics, input, or native window resizing.**
The existing session_core and GUI acceptance tests remain responsible for
those layers. In particular, changing the harness PTY width is not evidence
that ztty's native-window resize works.

## Run

Use the repository's Rust 1.100.0 toolchain and Linux native build headers
listed in `.github/workflows/termlens.yml`:

```sh
cargo build --locked --release --bin ztty
cargo fmt --manifest-path tests/termlens/Cargo.toml -- --check
cargo clippy --locked --manifest-path tests/termlens/Cargo.toml --all-targets -- -D warnings
ZTTY_BINARY="$PWD/target/release/ztty" \
  TERMLENS_ARTIFACT_DIR="$PWD/artifacts/termlens" \
  timeout 60s cargo test --locked --manifest-path tests/termlens/Cargo.toml -- --test-threads=1
```

`ZTTY_BINARY` must be absolute; it intentionally has no fallback to a PATH
binary. The version assertion reads the root package manifest so normal ztty
version bumps do not need a duplicate update here.

## Toolchain and CI scope

termlens is fixed at source commit
`d8a16d6c9d8691f8c61896f40bceb273806cd42c` (package version 0.11.10).
Its transitive dependencies are recorded in this directory's `Cargo.lock`;
`default-features = false` avoids unused insta snapshot support. Nothing is
added to the root manifest, lockfile, or ztty runtime dependency graph.

Official upstream references at the pinned revision:

- [Usage and limitations](https://github.com/vyncint/termlens/blob/d8a16d6c9d8691f8c61896f40bceb273806cd42c/README.md)
- [Test-writing skill](https://github.com/vyncint/termlens/blob/d8a16d6c9d8691f8c61896f40bceb273806cd42c/skills/termlens/SKILL.md)
- [Release workflow](https://github.com/vyncint/termlens/blob/d8a16d6c9d8691f8c61896f40bceb273806cd42c/.github/workflows/release.yml)

Upstream publishes the library as `termlens` and the executable separately as
`termlens-cli` to crates.io; the CLI is installed with
`cargo install termlens-cli --locked`. This suite uses the library directly
from the requested immutable source instead of installing a floating CLI.

The dedicated PR/manual workflow has read-only contents permission, pinned
action revisions, a 15-minute job limit and a 60-second test-command limit.
It builds ztty with the existing root lockfile and this harness with its own
lockfile. Native headers come from Ubuntu 24.04's package repositories as in
existing ztty CI; the runner image and OS packages are not bit-for-bit pinned.
It uploads only failing-wait diagnostics, does not run upstream report code,
does not publish anything, and needs no secrets or write permission.

This is independent of #27: it updates no submodules, dependency source pins
in the root workspace, or graphics temporary-file cleanup fixtures.
