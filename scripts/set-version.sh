#!/usr/bin/env bash
# Sets the release version everywhere it is pinned. tests/packaging.rs and
# tests/gate.rs fail when these disagree.
#   scripts/set-version.sh 0.2.0
# Needs GNU sed (the release workflow runs it on ubuntu).
set -euo pipefail

version="${1:?usage: set-version.sh <version>}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not X.Y.Z: $version" >&2; exit 1; }
cd "$(dirname "$0")/.."

sed -i -E '0,/^version = "[^"]*"/s//version = "'"$version"'"/' Cargo.toml pyproject.toml
cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace

sed -i -E '0,/"version": "[^"]*"/s//"version": "'"$version"'"/' \
    package.json \
    packaging/claude/.claude-plugin/plugin.json \
    packaging/codex/.codex-plugin/plugin.json \
    packaging/copilot/.github/plugin/plugin.json \
    packaging/cursor/.cursor-plugin/plugin.json \
    packaging/npm/oncebitten/package.json

sed -i -E 's/^version=.*/version='"$version"'/' packaging/*/bin/oncebitten
sed -i -E 's#(/oncebitten/)v[0-9.]+(/install\.sh)#\1v'"$version"'\2#; s/(ONCEBITTEN_VERSION=)[0-9.]+ /\1'"$version"' /' templates/*.yml
sed -i -E 's/(oncebitten==)[0-9.]+/\1'"$version"'/' .pre-commit-hooks.yaml
sed -i -E 's/(oncebitten==)[0-9.]+/\1'"$version"'/; s/(oncebitten@)[0-9.]+/\1'"$version"'/; s/("oncebitten": ")[0-9.]+"/\1'"$version"'"/; s/(rev: v)[0-9.]+/\1'"$version"'/' README.md docs/install.md
