#!/bin/sh
set -eu

# Use the same opt-in frontend feature for the test and release executable.
if [ "${ZTTY_WAYLAND:-0}" = 1 ]; then
    set -- --features wayland
else
    set --
fi

python3 tests/registry_versions_test.py
python3 tests/source_archive_test.py
cargo fmt --all -- --check
# Independent reusable crates keep their own workspace and validation scope.
for package in zgpui zinput zpty zgraphics zterm; do
    cargo fmt --manifest-path "crates/$package/Cargo.toml" --all -- --check
    cargo test --locked --manifest-path "crates/$package/Cargo.toml"
    cargo clippy --locked --manifest-path "crates/$package/Cargo.toml" --all-targets -- -D warnings
done
cargo test --locked --workspace "$@"
cargo clippy --locked --workspace --all-targets "$@" -- -D warnings
cargo build --locked --release "$@"
python3 tools/check_no_std.py
if [ "$(uname -s)" = Linux ]; then
    if [ "${ZTTY_WAYLAND:-0}" = 1 ]; then
        python3 tests/wayland_services.py --sanitize --output artifacts/wayland-services
        python3 tests/build_wayland_ime_peer.py --output artifacts/wayland-ime-peer
    fi
    python3 tests/install_test.py
    python3 tests/bash_integration.py --output artifacts/bash-integration/report.json
    python3 tests/mcp_transport.py --binary ./target/release/ztty
    python3 tests/mcp_sessions.py --binary ./target/release/ztty
    python3 tests/mcp_security.py --binary ./target/release/ztty
    python3 tests/mcp_wait.py --binary ./target/release/ztty --output artifacts/mcp-wait/report.json
    python3 tests/mcp_sse.py --binary ./target/release/ztty --output artifacts/mcp-sse/report.json
else
    printf '%s\n' 'Shared-session MCP checks: skipped (Linux-only runtime).'
fi

# The live MCP checks target Linux. A bare-metal build is an additional check
# that the portable libraries do not acquire operating-system dependencies.
# Opt in after installing the target; never silently label a skipped gate passed.
if [ "${ZTTY_BARE_METAL:-0}" = 1 ]; then
    cargo check --locked --manifest-path crates/zterm/Cargo.toml --lib --target x86_64-unknown-none
else
    printf '%s\n' 'Bare-metal portability check: skipped (set ZTTY_BARE_METAL=1 to require it).'
fi
