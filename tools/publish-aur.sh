#!/usr/bin/env bash
set -euo pipefail

package=${1:?usage: tools/publish-aur.sh GENERATED_AUR_DIR}
scratch=$(mktemp -d)
trap 'python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1])" "$scratch"' EXIT

if [[ -n ${AUR_SSH_KEY:-} ]]; then
    umask 077
    printf '%s\n' "$AUR_SSH_KEY" > "$scratch/key"
    unset AUR_SSH_KEY
    export GIT_SSH_COMMAND="ssh -F /dev/null -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$PWD/packaging/aur/known_hosts -i $scratch/key"
fi

git -c init.defaultBranch=master clone ssh://aur@aur.archlinux.org/ztty.git "$scratch/aur"
target_version=$(sed -n 's/^[[:space:]]*pkgver = //p' "$package/.SRCINFO")
current_version=$(sed -n 's/^[[:space:]]*pkgver = //p' "$scratch/aur/.SRCINFO" 2>/dev/null || true)
if [[ -n $current_version && $current_version != "$target_version" &&
    $(printf '%s\n' "$current_version" "$target_version" | sort -V | tail -n1) == "$current_version" ]]; then
    echo "AUR already has a newer version ($current_version)."
    exit 0
fi
cp "$package/PKGBUILD" "$package/.SRCINFO" "$scratch/aur/"
git -C "$scratch/aur" add PKGBUILD .SRCINFO
if ! git -C "$scratch/aur" diff --cached --quiet; then
    git -C "$scratch/aur" -c user.name=LIghtJUNction \
        -c user.email=lightjunction.me@gmail.com commit -m "Update ztty to $target_version"
    git -C "$scratch/aur" push origin HEAD:master
fi
