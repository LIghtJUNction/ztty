#!/bin/sh
set -eu

case $(uname -s) in
    Linux) ;;
    Darwin)
        echo 'Native macOS builds (Apple Silicon and Intel) are planned.' >&2
        exit 1 ;;
    *)
        echo 'This installer currently supports Linux. Native Windows builds are planned.' >&2
        exit 1 ;;
esac

case $(uname -m) in
    x86_64|amd64) arch=x86_64 ;;
    aarch64|arm64) arch=aarch64 ;;
    *) echo 'ztty supports Linux x86_64 and aarch64.' >&2; exit 1 ;;
esac

base=https://github.com/LIghtJUNction/ztty/releases
version=${ZTTY_VERSION:-latest}
case "$version" in
    latest) assets=$base/latest/download ;;
    *)
        if ! printf '%s\n' "$version" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$'; then
            echo 'ZTTY_VERSION must be latest or a version tag such as v0.1.1.' >&2
            exit 1
        fi
        assets=$base/download/$version ;;
esac

asset=ztty-linux-$arch.tar.gz
tmp=$(mktemp -d)
trap 'unlink "$tmp/$asset" 2>/dev/null || :; unlink "$tmp/$asset.sha256" 2>/dev/null || :; unlink "$tmp/ztty" 2>/dev/null || :; rmdir "$tmp"' EXIT
curl -fsSL --retry 3 "$assets/$asset" -o "$tmp/$asset"
curl -fsSL --retry 3 "$assets/$asset.sha256" -o "$tmp/$asset.sha256"
read -r expected _ < "$tmp/$asset.sha256"
actual=$(sha256sum "$tmp/$asset")
actual=${actual%% *}
if [ "$expected" != "$actual" ]; then
    echo 'ztty archive checksum mismatch.' >&2
    exit 1
fi
tar -xzf "$tmp/$asset" -C "$tmp" ztty
if ! "$tmp/ztty" --version; then
    echo 'ztty requires glibc and the X11, Xft, and Xrender runtime libraries.' >&2
    exit 1
fi

destination=${ZTTY_INSTALL_DIR:-$HOME/.local/bin}
mkdir -p "$destination"
install -m755 "$tmp/ztty" "$destination/ztty"
printf 'Installed %s\n' "$destination/ztty"
case ":$PATH:" in
    *":$destination:"*) ;;
    *) printf 'Add %s to PATH to run ztty by name.\n' "$destination" ;;
esac
