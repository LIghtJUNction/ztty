#!/bin/sh
set -eu

fail() {
    printf '%s\n' "$*" >&2
    exit 1
}

source_binary=
case $# in
    0) ;;
    1)
        [ "$1" = --help ] || fail 'Usage: sh install.sh [--from-binary PATH]'
        cat <<'USAGE'
Usage: sh install.sh [--from-binary PATH]

Without arguments, download and verify the latest Linux release.
--from-binary installs an already built native executable without downloading.

ZTTY_VERSION      Release tag (for example v0.2.1), or latest (default).
ZTTY_INSTALL_DIR  Binary directory (default: $HOME/.local/bin).
ZTTY_DESKTOP_DIR  Launcher directory (default: $XDG_DATA_HOME/applications,
                  or $HOME/.local/share/applications).
DESTDIR           Optional package staging root; launchers use the final path.

An upgrade keeps one verified binary or original symlink at ztty.previous.
USAGE
        exit 0 ;;
    2)
        [ "$1" = --from-binary ] || fail 'Usage: sh install.sh [--from-binary PATH]'
        [ -n "$2" ] || fail '--from-binary requires a path.'
        source_binary=$2 ;;
    *) fail 'Usage: sh install.sh [--from-binary PATH]' ;;
esac

system=$(uname -s)
case "$system" in
    Linux) ;;
    Darwin)
        [ -n "$source_binary" ] || fail 'Use a native macOS build with --from-binary; release downloads here support Linux.' ;;
    *) fail 'This installer supports Linux releases and local Linux/macOS builds.' ;;
esac

# The BSD and GNU options both replace the link itself, including a link to a
# directory. Staging in the target directory keeps the rename on one filesystem.
atomic_move() {
    if [ "$system" = Darwin ]; then
        mv -fh "$1" "$2"
    else
        mv -fT -- "$1" "$2"
    fi
}

digest() {
    if [ "$system" = Darwin ]; then
        checksum=$(shasum -a 256 < "$1") || return 1
    else
        checksum=$(sha256sum < "$1") || return 1
    fi
    printf '%s\n' "${checksum%% *}"
}

native_binary() {
    [ -f "$1" ] && [ ! -L "$1" ] || return 1
    magic=$(od -An -tx1 -N4 "$1" | tr -d ' \n')
    case "$system:$magic" in
        Linux:7f454c46 | Darwin:feedface | Darwin:cefaedfe | \
        Darwin:feedfacf | Darwin:cffaedfe | Darwin:cafebabe | \
        Darwin:bebafeca | Darwin:cafebabf | Darwin:bfbafeca) return 0 ;;
        *) return 1 ;;
    esac
}

temporary=
binary_stage=
desktop_stage=
backup_stage=
checksum_stage=
cleanup() {
    for staged in "$binary_stage" "$desktop_stage" "$backup_stage" "$checksum_stage"; do
        [ -z "$staged" ] || rm -f -- "$staged"
    done
    [ -z "$temporary" ] || rm -rf -- "$temporary"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM

version=${ZTTY_VERSION:-latest}
if [ -z "$source_binary" ]; then
    case $(uname -m) in
        x86_64|amd64) arch=x86_64 ;;
        aarch64|arm64) arch=aarch64 ;;
        *) fail 'ztty supports Linux x86_64 and aarch64.' ;;
    esac
    base=https://github.com/LIghtJUNction/ztty/releases
    case "$version" in
        latest) assets=$base/latest/download ;;
        *)
            case "$version" in *'
'*) fail 'ZTTY_VERSION must be a single version tag.' ;; esac
            printf '%s\n' "$version" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' ||
                fail 'ZTTY_VERSION must be latest or a version tag such as v0.2.1.'
            assets=$base/download/$version ;;
    esac
    asset=ztty-linux-$arch.tar.gz
    temporary=$(mktemp -d)
    curl -fsSL --retry 3 "$assets/$asset" -o "$temporary/$asset"
    curl -fsSL --retry 3 "$assets/$asset.sha256" -o "$temporary/$asset.sha256"
    read -r expected _ < "$temporary/$asset.sha256"
    printf '%s\n' "$expected" | grep -Eq '^[0-9a-fA-F]{64}$' ||
        fail 'ztty archive checksum is invalid.'
    expected=$(printf '%s' "$expected" | tr A-F a-f)
    archive_hash=$(digest "$temporary/$asset") || fail 'Could not calculate the archive SHA256.'
    [ "$expected" = "$archive_hash" ] ||
        fail 'ztty archive checksum mismatch.'
    tar -xzf "$temporary/$asset" -C "$temporary" ztty
    source_binary=$temporary/ztty
fi
native_binary "$source_binary" ||
    fail 'ztty must be a regular native executable, not a directory, script, or symlink.'

destination=${ZTTY_INSTALL_DIR:-$HOME/.local/bin}
desktop_directory=${ZTTY_DESKTOP_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/applications}
staging_root=${DESTDIR:-}
case "$destination" in /*) ;; *) destination=$(pwd -P)/$destination ;; esac
case "$desktop_directory" in /*) ;; *) desktop_directory=$(pwd -P)/$desktop_directory ;; esac
case "$staging_root" in ''|/*) ;; *) staging_root=$(pwd -P)/$staging_root ;; esac
# Desktop entries cannot safely represent control characters in these paths.
case "$destination$desktop_directory$staging_root" in *'
'*) fail 'Installation paths must not contain control characters.' ;; esac
if printf '%s' "$destination$desktop_directory$staging_root" | LC_ALL=C grep -q '[[:cntrl:]]'; then
    fail 'Installation paths must not contain control characters.'
fi
binary_directory=$staging_root$destination
launcher_directory=$staging_root$desktop_directory
target=$binary_directory/ztty
desktop_target=$launcher_directory/ztty.desktop
backup=$binary_directory/ztty.previous
backup_checksum=$binary_directory/ztty.previous.sha256
for output in "$target" "$desktop_target" "$backup" "$backup_checksum"; do
    if [ -e "$output" ] && [ ! -f "$output" ] && [ ! -L "$output" ]; then
        fail "Refusing to replace a directory or special file: $output"
    fi
done
[ ! "$source_binary" -ef "$target" ] ||
    fail 'The source executable is already the installation target.'
if [ -e "$target" ] && [ ! -L "$target" ]; then
    native_binary "$target" || fail "Refusing to replace a non-native file: $target"
fi

mkdir -p "$binary_directory" "$launcher_directory"
binary_stage=$(mktemp "$binary_directory/.ztty-install.XXXXXXXX")
install -m755 "$source_binary" "$binary_stage"
source_hash=$(digest "$source_binary") || fail 'Could not calculate the source executable SHA256.'
staged_hash=$(digest "$binary_stage") || fail 'Could not calculate the staged executable SHA256.'
[ "$source_hash" = "$staged_hash" ] ||
    fail 'ztty staging checksum mismatch.'
if ! installed_version=$("$binary_stage" --version); then
    fail 'ztty could not run. Check the native architecture and required runtime libraries (Linux: glibc, X11, Xft, Xrender).'
fi
case "$installed_version" in *'
'*) fail 'The executable returned an invalid ztty version.' ;; esac
printf '%s\n' "$installed_version" | grep -Eq '^ztty [0-9]+\.[0-9]+\.[0-9]+$' ||
    fail 'The executable returned an invalid ztty version.'
if [ -n "$temporary" ] && [ "$version" != latest ]; then
    [ "$installed_version" = "ztty ${version#v}" ] ||
        fail 'The downloaded executable version does not match ZTTY_VERSION.'
fi

# Exec has desktop-string escaping followed by argument unquoting. Use the
# absolute installed path so an older system ztty cannot win PATH lookup.
desktop_exec=$(printf '%s' "$destination/ztty" | sed \
    -e 's/\\/\\\\\\\\/g' -e 's/"/\\\\"/g' \
    -e 's/`/\\\\`/g' -e 's/\$/\\\\$/g' -e 's/%/%%/g')
desktop_command="\"$desktop_exec\""
case "$destination" in
    *%*|*=*)
        # Launchers may resolve argv[0] before expanding %% and the desktop
        # format disallows '=' in argv[0]. Keep unusual paths in a data argument
        # to a fixed command; the shell never interprets the path as code.
        desktop_command='/bin/sh -c "exec \\"\\$1\\"" ztty "'"$desktop_exec"'"' ;;
esac
desktop_stage=$(mktemp "$launcher_directory/.ztty-desktop.XXXXXXXX")
cat > "$desktop_stage" <<DESKTOP
[Desktop Entry]
Type=Application
Name=ztty
GenericName=Terminal
Comment=A lean terminal built on Project Z
Exec=$desktop_command
Icon=utilities-terminal
Terminal=false
Categories=System;TerminalEmulator;
Keywords=terminal;shell;console;
StartupWMClass=Ztty
DESKTOP
chmod 644 "$desktop_stage"

# Keep a single rollback slot. Reinstalling identical bytes preserves an older
# rollback copy. A previous symlink is copied as a link, never dereferenced.
if [ -L "$target" ]; then
    backup_stage=$(mktemp "$binary_directory/.ztty-backup.XXXXXXXX")
    rm -f -- "$backup_stage"
    cp -P "$target" "$backup_stage"
    [ -L "$backup_stage" ] && [ "$(readlink "$target")" = "$(readlink "$backup_stage")" ] ||
        fail 'ztty symlink backup verification failed.'
    atomic_move "$backup_stage" "$backup"
    backup_stage=
    rm -f -- "$backup_checksum"
    printf 'Previous symlink preserved at %s\n' "$backup"
elif [ -f "$target" ] && ! cmp -s "$target" "$binary_stage"; then
    backup_stage=$(mktemp "$binary_directory/.ztty-backup.XXXXXXXX")
    previous_hash=$(digest "$target") || fail 'Could not calculate the previous executable SHA256.'
    install -m755 "$target" "$backup_stage"
    backup_hash=$(digest "$backup_stage") || fail 'Could not calculate the backup SHA256.'
    [ "$previous_hash" = "$backup_hash" ] ||
        fail 'ztty backup checksum mismatch.'
    checksum_stage=$(mktemp "$binary_directory/.ztty-checksum.XXXXXXXX")
    printf '%s  ztty.previous\n' "$previous_hash" > "$checksum_stage"
    chmod 644 "$checksum_stage"
    atomic_move "$backup_stage" "$backup"
    backup_stage=
    atomic_move "$checksum_stage" "$backup_checksum"
    checksum_stage=
    printf 'Previous binary preserved at %s (SHA256 %s)\n' "$backup" "$previous_hash"
fi
atomic_move "$binary_stage" "$target"
binary_stage=
atomic_move "$desktop_stage" "$desktop_target"
desktop_stage=
printf 'Installed %s at %s\nLauncher: %s\n' "$installed_version" "$target" "$desktop_target"
case ":$PATH:" in
    *":$destination:"*) ;;
    *) printf 'Add %s to PATH to run ztty by name.\n' "$destination" ;;
esac
