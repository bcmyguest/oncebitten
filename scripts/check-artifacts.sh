#!/usr/bin/env bash
# Fails unless every release tarball and wheel under <dir> ships exactly the
# binaries oncebitten and oncebitten-checker: test and fake binaries never ship.
#   scripts/check-artifacts.sh <dir>
set -euo pipefail

dir="${1:?usage: check-artifacts.sh <dir>}"
want=$'oncebitten\noncebitten-checker'
checked=0

names() {
    sed -E 's#.*/##; s/\.exe$//' | grep -v '^$' | sort
}

while IFS= read -r archive; do
    got="$(tar -tzf "$archive" | names)"
    [ "$got" = "$want" ] || { echo "$archive ships:"$'\n'"$got" >&2; exit 1; }
    checked=$((checked + 1))
done < <(find "$dir" -name '*.tar.gz')

while IFS= read -r wheel; do
    got="$(unzip -Z1 "$wheel" | grep '\.data/scripts/' | names)"
    [ "$got" = "$want" ] || { echo "$wheel ships:"$'\n'"$got" >&2; exit 1; }
    checked=$((checked + 1))
done < <(find "$dir" -name '*.whl')

[ "$checked" -gt 0 ] || { echo "no tarballs or wheels under $dir" >&2; exit 1; }
echo "$checked artifacts ship only oncebitten and oncebitten-checker"
