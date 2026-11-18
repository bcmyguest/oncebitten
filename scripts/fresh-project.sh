#!/usr/bin/env bash
# Installs oncebitten into a fresh git repository the way a user would, then
# runs a smoke flow against it.
#
#   scripts/fresh-project.sh artifacts <dir>      local wheel and npm tarballs
#   scripts/fresh-project.sh published <version>  PyPI and npm, after a release
#
# <dir> holds exactly one wheel, the npm tarball oncebitten-<v>.tgz and the
# release tarball oncebitten-<target>.tar.gz with its .sha256 for this machine;
# the npm launcher fetches that release tarball from a file:// release layout.
# CHANNELS selects what to test (default "pip npm"). Needs git, uv, node and npm.
# Every install lands in a temporary directory; nothing touches the real user
# config, uv tool dir or global npm prefix.
set -euo pipefail

usage="usage: fresh-project.sh artifacts <dir> | published <version>"
mode="${1:?$usage}"
arg="${2:?$usage}"
channels="${CHANNELS:-pip npm}"

[ "$mode" = artifacts ] && arg="$(cd "$arg" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

native() {
    if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s\n' "$1"; fi
}

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

step() {
    echo "--- $*"
}

find_one() {
    local found
    found=("$arg"/$1)
    [ "${#found[@]}" = 1 ] && [ -f "${found[0]}" ] || fail "want exactly one $1 in $arg"
    native "${found[0]}"
}

export HOME="$work/home"
export XDG_CONFIG_HOME="$HOME/.config" XDG_DATA_HOME="$HOME/.local/share"
mkdir -p "$HOME"
export GIT_CONFIG_GLOBAL="$work/gitconfig" GIT_CONFIG_SYSTEM=/dev/null
git config --global user.email smoke@example.invalid
git config --global user.name smoke
git config --global init.defaultBranch main
git config --global commit.gpgsign false

# Prints the directory holding the oncebitten and oncebitten-checker commands.
install_pip() {
    export UV_TOOL_DIR="$work/uv-tools" UV_TOOL_BIN_DIR="$work/uv-bin"
    case "$mode" in
        artifacts) uv tool install "$(find_one '*.whl')" >&2 ;;
        published) uv tool install "oncebitten==$arg" >&2 ;;
    esac
    echo "$UV_TOOL_BIN_DIR"
}

# Runs in this shell, not a subshell: the launcher reads the exported variables.
serve_release() {
    local release version archive url
    archive="$(find_one 'oncebitten-*.tar.gz')"
    version="$(tar -xzOf "$(find_one 'oncebitten-[0-9]*.tgz')" package/package.json | node -p 'JSON.parse(require("fs").readFileSync(0, "utf8")).version')"
    release="$work/release/download/v$version"
    mkdir -p "$release"
    cp "$archive" "$archive.sha256" "$release/"
    url="$(native "$work/release")"
    case "$url" in /*) ;; *) url="/$url" ;; esac
    ONCEBITTEN_RELEASE_URL="file://$url"
    ONCEBITTEN_CACHE_DIR="$(native "$work/npm-cache")"
    export ONCEBITTEN_RELEASE_URL ONCEBITTEN_CACHE_DIR
}

install_npm() {
    local project="$work/npm-project"
    mkdir -p "$project"
    echo '{"name":"fresh","private":true}' >"$project/package.json"
    if [ "$mode" = artifacts ]; then
        (cd "$project" && npm install --no-audit --no-fund "$(find_one 'oncebitten-[0-9]*.tgz')" >&2)
    else
        (cd "$project" && npm install --no-audit --no-fund "oncebitten@$arg" >&2)
    fi
    echo "$project/node_modules/.bin"
}

smoke() {
    local name="$1" bin_dir="$2" want="$3" repo="$work/repo-$1"
    export PATH="$bin_dir:$PATH"
    step "$name: oncebitten --version"
    echo "using $(command -v oncebitten || fail "$name: oncebitten not on PATH")"
    command -v oncebitten-checker >/dev/null || fail "$name: oncebitten-checker not on PATH"
    local version
    version="$(oncebitten --version)"
    echo "$version"
    [ -z "$want" ] || [[ "$version" == *"$want"* ]] || fail "$name: version '$version' lacks '$want'"

    mkdir -p "$repo"
    cd "$repo"
    git init -q
    echo smoke >README
    git add README
    git commit -q -m "init" -m "Assisted-by: Claude:claude-sonnet-5-5"

    step "$name: hooks install --scope project"
    oncebitten hooks install --host claude --scope project
    test -f .claude/settings.json || fail "$name: no .claude/settings.json"

    step "$name: capture the same mistake in two sessions"
    local cwd="$repo" session
    if pwd -W >/dev/null 2>&1; then cwd="$(pwd -W)"; fi
    for session in s1 s2; do
        printf '{"session_id":"%s","cwd":"%s","hook_event_name":"PostToolUseFailure","tool_name":"Bash","tool_input":{"command":"pytest tests/test_app.py"},"error":"Exit code 1\\n1 failed","is_interrupt":false,"tool_use_id":"t-%s"}' \
            "$session" "$cwd" "$session" | oncebitten capture --host claude
    done

    step "$name: evidence list"
    local evidence
    evidence="$(oncebitten evidence list)"
    echo "$evidence"
    [[ "$evidence" == *"sessions=2"* ]] || fail "$name: no cluster with sessions=2"

    step "$name: check"
    oncebitten check

    step "$name: gate pre-push"
    oncebitten gate pre-push </dev/null
    cd "$work"
}

want=""
[ "$mode" = published ] && want="$arg"
for channel in $channels; do
    case "$channel" in
        pip) smoke pip "$(install_pip)" "$want" ;;
        npm)
            [ "$mode" = artifacts ] && serve_release
            smoke npm "$(install_npm)" "$want"
            ;;
        *) fail "unknown channel $channel" ;;
    esac
done
echo "fresh-project: all channels passed ($channels)"
