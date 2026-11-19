#!/usr/bin/env bash
set -euo pipefail

out=${1:?usage: tools/release-source.sh OUTPUT_DIR}
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Expected a stable X.Y.Z workspace version" >&2
    exit 1
fi
if [[ ${GITHUB_REF:-} == refs/tags/* && ${GITHUB_REF_NAME:-} != "v$version" ]]; then
    echo "Tag does not match Cargo.toml version $version" >&2
    exit 1
fi
name=ztty-$version
mkdir -p "$out/aur"
python3 tools/source_archive.py --prefix "$name" --output "$out/$name.tar.gz"
digest=$(sha256sum "$out/$name.tar.gz" | cut -d ' ' -f 1)
sed -e "s/@VERSION@/$version/g" -e "s/@SHA256@/$digest/g" \
    packaging/aur/PKGBUILD.in > "$out/aur/PKGBUILD"
(
    cd "$out"
    sha256sum "$name.tar.gz" > SHA256SUMS
)
printf '%s\n' "$version"
