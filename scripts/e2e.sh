#!/usr/bin/env bash
# End-to-end run of a release binary in a scratch repo: rules, replay (OpenShell),
# check, the pre-push gate through pre-commit, exceptions, baseline, stale proofs,
# rollback and retire, steer, capture, hooks install, proposals and doctor.
# Every step states the exit code it expects; the script exits 1 if any differs.
# No agent host runs here: scripts/e2e-live.sh covers background triage.
#
#   cargo build --release && scripts/e2e.sh [<oncebitten source dir>]
#
# Needs git, pre-commit and an OpenShell gateway. HOME and the XDG dirs point at
# a scratch directory, so the real user config stays untouched.
set -u
SRC=$(cd "${1:-$(dirname "$0")/..}" && pwd)
export PATH=$SRC/target/release:$PATH
ROOT=$(mktemp -d); trap 'rm -rf "$ROOT"' EXIT; mkdir -p $ROOT/home/.config
for d in $HOME/.config/openshell; do [ -d "$d" ] && cp -r "$d" $ROOT/home/.config/; done
export HOME=$ROOT/home XDG_DATA_HOME=$ROOT/home/.local/share XDG_CONFIG_HOME=$ROOT/home/.config
export PRE_COMMIT_HOME=$ROOT/pc-cache
git config --global user.email e2e@example.invalid; git config --global user.name e2e; git config --global init.defaultBranch main
TRAILER="Assisted-by: Claude:claude-fable-5-2"
ci() { git commit -q -m "$1" -m "$TRAILER"; }
pass=0; fail=0
ok() { pass=$((pass+1)); echo "PASS $1"; }
ko() { fail=$((fail+1)); echo "FAIL $1"; }
expect() { local want=$1 name=$2; shift 2; local out; out=$("$@" 2>&1); local got=$?
  if [ "$got" = "$want" ]; then ok "[$got] $name"; else ko "[want $want got $got] $name"; echo "$out" | sed 's/^/    | /' | head -15; fi; LAST="$out"; }
show() { echo "$LAST" | sed 's/^/    > /' | head -${1:-6}; }

git init -q --bare $ROOT/remote.git
git init -q $ROOT/repo && cd $ROOT/repo && git remote add origin $ROOT/remote.git
mkdir -p .oncebitten/rules src
for r in py-utcnow no-sudo arch-diagram; do cp -r $SRC/examples/$r .oncebitten/rules/; sed -i 's/^status = "proposed"/status = "active"/' .oncebitten/rules/$r/rule.toml; done
printf 'import datetime\n\ndef now():\n    return datetime.datetime.now(datetime.UTC)\n' > src/app.py
cat > .pre-commit-config.yaml <<YAML
repos:
- repo: $SRC
  rev: $(git -C $SRC rev-parse HEAD)
  hooks:
  - id: oncebitten-gate-system
YAML
git add -A && ci init && git push -q -u origin main

echo "== rules"
expect 0 "rules validate" oncebitten rules validate
expect 0 "rules list" oncebitten rules list; show 8

echo "== replay (OpenShell)"
for r in py-utcnow arch-diagram no-sudo; do expect 0 "replay $r" oncebitten replay $r; show 3; done
expect 0 "rules sync" oncebitten rules sync
git add -A && ci proofs && git push -q

echo "== check"
expect 0 "check clean tree" oncebitten check
printf 'import datetime\nstamp = datetime.datetime.utcnow()\n' > src/bad.py
expect 1 "check fires on utcnow" oncebitten check; show 3

echo "== gate pre-push via pre-commit"
expect 0 "pre-commit install pre-push" pre-commit install --hook-type pre-push
git add src/bad.py && ci bad
expect 1 "git push blocked" git push -q; show 8
expect 1 "gate pr blocked" oncebitten gate pr --base origin/main; show 4

echo "== exception"
expect 0 "exception add" oncebitten exception add py-utcnow --paths src/bad.py --reason "legacy module" --expires 2099-01-01
show 3
expect 0 "exception list" oncebitten exception list; show 3
expect 0 "gate pr passes with exception" oncebitten gate pr --base origin/main; show 3
rm -f .oncebitten/exceptions.toml

echo "== baseline"
expect 0 "baseline update" oncebitten baseline update
git add -A && ci baseline
expect 0 "git push passes with baseline" git push -q; show 4

echo "== proofs: stale after rule edit"
echo "# edited" >> .oncebitten/rules/py-utcnow/check.yml
expect 2 "gate pr fails on stale proof" oncebitten gate pr --base origin/main; show 3
git checkout -q .oncebitten/rules/py-utcnow/check.yml

echo "== retire / rollback"
expect 0 "rules rollback py-utcnow" oncebitten rules rollback py-utcnow; show 3
expect 2 "rollback again (not active)" oncebitten rules rollback py-utcnow
sed -i 's/^status = "proposed"/status = "active"/' .oncebitten/rules/py-utcnow/rule.toml
expect 0 "re-activated rule needs no new replay" oncebitten gate pr --base origin/main
expect 0 "rules retire arch-diagram" oncebitten rules retire arch-diagram --reason "e2e"
grep -q 'status = "retired"' .oncebitten/rules/arch-diagram/rule.toml && ok "retired status written" || ko "retired status"
git checkout -q .oncebitten/rules/arch-diagram/rule.toml

echo "== steer"
deny='{"session_id":"s","cwd":"'$PWD'","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"sudo rm -rf /tmp/x"}}'
fine='{"session_id":"s","cwd":"'$PWD'","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}'
out=$(echo "$deny" | oncebitten steer --host claude); echo "$out" | grep -q '"deny"' && ok "steer denies sudo" || ko "steer deny: $out"
out=$(echo "$fine" | oncebitten steer --host claude); [ -z "$out" ] && ok "steer silent on ls" || ko "steer on ls: $out"

echo "== capture + evidence"
for s in s1 s2; do
  echo '{"session_id":"'$s'","cwd":"'$PWD'","hook_event_name":"PostToolUseFailure","tool_name":"Bash","tool_input":{"command":"pytest tests/test_app.py"},"error":"Exit code 1\n1 failed","is_interrupt":false,"tool_use_id":"t-'$s'"}' | oncebitten capture --host claude
done
echo '{"session_id":"s3","cwd":"'$PWD'","hook_event_name":"UserPromptSubmit","prompt":"never push to main directly"}' | oncebitten capture --host claude
expect 0 "evidence list --all" oncebitten evidence list --all; show 6

echo "== hooks install (scratch HOME)"
expect 0 "hooks install claude user" oncebitten hooks install --host claude --scope user; show 3
grep -q "steer" $HOME/.claude/settings.json && grep -q "capture" $HOME/.claude/settings.json && ok "settings hold capture and steer" || { ko "settings.json"; cat $HOME/.claude/settings.json; }

echo "== proposals"
expect 0 "proposals list (empty)" oncebitten proposals list; show 3
expect 2 "proposals accept unknown id" oncebitten proposals accept nope; show 2

echo "== doctor"
expect 0 "doctor" oncebitten doctor; show 30

echo; echo "RESULT pass=$pass fail=$fail"
[ "$fail" = 0 ]
