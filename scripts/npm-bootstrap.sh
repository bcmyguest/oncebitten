#!/usr/bin/env bash
# One-time setup for npm trusted publishing. Trusted publishing cannot create
# a package, so each package needs one placeholder publish by a signed-in
# owner, then a trust relationship with release.yml.
#
#   scripts/npm-bootstrap.sh            create missing packages, then trust release.yml
#   scripts/npm-bootstrap.sh --dry-run  print what would run
#
# Needs npm >= 11.15 and `npm login` as an owner with 2FA. Safe to run again:
# an existing package is not republished.
set -euo pipefail

REPO="bcmyguest/oncebitten"
WORKFLOW="release.yml"
ENVIRONMENT="npm"
PLACEHOLDER="0.0.0"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT/packaging/npm/oncebitten/package.json"

run() {
  if [ "$DRY_RUN" = 1 ]; then echo "would run: $*"; else "$@"; fi
}

publish_placeholder() {
  local name="$1" dir
  dir="$(mktemp -d)"
  cat > "$dir/package.json" <<JSON
{
  "name": "$name",
  "version": "$PLACEHOLDER",
  "description": "Placeholder. Releases of $name are published by GitHub Actions.",
  "license": "Apache-2.0",
  "repository": { "type": "git", "url": "git+https://github.com/$REPO.git" }
}
JSON
  (cd "$dir" && run npm publish --access public)
  rm -rf "$dir"
}

DRY_RUN=0
[ "${1:-}" = "--dry-run" ] && DRY_RUN=1

if [ "$DRY_RUN" = 0 ] && ! npm whoami > /dev/null 2>&1; then
  echo "error: not signed in to npm; run npm login" >&2
  exit 2
fi

names="$(node -p 'const m = require(process.argv[1]); [m.name, ...Object.keys(m.optionalDependencies)].join("\n")' "$MANIFEST")"

failed=0
for name in $names; do
  if npm view "$name" name > /dev/null 2>&1; then
    echo "$name: exists"
  else
    echo "$name: publishing placeholder $PLACEHOLDER"
    publish_placeholder "$name"
  fi
  # npm refuses a second identical relationship; report it and go on.
  if ! run npm trust github "$name" --file "$WORKFLOW" --repo "$REPO" --env "$ENVIRONMENT" --allow-publish --yes; then
    echo "$name: trust not created; check npm trust list $name" >&2
    failed=1
  fi
done
exit "$failed"
