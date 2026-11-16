#!/usr/bin/env bash
# Live background triage with a real agent host. Two real Claude Code sessions
# each write datetime.utcnow() and get corrected; the project capture hook
# records both corrections, the first one starts triage in the background, and
# the script waits for the proposal, accepts it and checks that the gate passes
# existing code and fails a new copy of the rule's bad fixture. A fifth session
# asks for `sudo -n true` and expects the shipped no-sudo steer rule to deny it.
# Spends model calls: five `claude -p` turns plus one triage call.
#
#   cargo build --release && scripts/e2e-live.sh [<host>:<agent>]
#
# <host>:<agent> is the triage target (default claude:haiku; codex:<model> asks
# codex). The sessions always run in Claude Code. Needs git, claude signed in,
# the triage host signed in and an OpenShell gateway. The evidence store and
# config live in a scratch directory; HOME stays real so the hosts keep their login.
# The sessions load project and local settings only: a user-scope oncebitten hook
# from another build would race this one and stamp the proof with its own checker.
set -u
TARGET=${1:-claude:haiku}
SRC=$(cd "$(dirname "$0")/.." && pwd)
export PATH=$SRC/target/release:$PATH
ROOT=$(mktemp -d)
mkdir -p "$ROOT/data" "$ROOT/config/oncebitten"
[ -d "$HOME/.config/openshell" ] && cp -r "$HOME/.config/openshell" "$ROOT/config/"
export XDG_DATA_HOME=$ROOT/data XDG_CONFIG_HOME=$ROOT/config
export ONCEBITTEN_TRIAGE_DEFAULT=$TARGET ONCEBITTEN_TRIAGE_CLAUDE=
WAIT_SECONDS=300
pass=0; fail=0
ok() { pass=$((pass+1)); echo "PASS $1"; }
ko() { fail=$((fail+1)); echo "FAIL $1"; }
turn() { timeout 240 claude -p "$2" --model haiku --permission-mode acceptEdits \
  --setting-sources project,local --allowedTools Edit,Write,Read $1 --output-format json >/dev/null 2>>"$ROOT/claude.err"; }

git init -q "$ROOT/repo" && cd "$ROOT/repo" || exit 1
mkdir src && printf 'import datetime\n\ndef stamp():\n    return datetime.datetime.now(datetime.UTC)\n' > src/app.py
oncebitten hooks install --host claude --scope project >/dev/null && ok "hooks install" || ko "hooks install"
commit() { git -c user.email=e2e@example.invalid -c user.name=e2e commit -q -m "$1" \
  -m "Assisted-by: Claude:claude-fable-5-2"; }
git add -A && commit init

for n in a b; do
  id=$(python3 -c "import uuid;print(uuid.uuid4())")
  turn "--session-id $id" "Append a function named created_$n() to src/app.py that returns datetime.datetime.utcnow(). Change nothing else. Reply with the word done." \
    && ok "session $n: edit" || ko "session $n: edit"
  turn "--resume $id" "no, never use datetime.utcnow(); it is deprecated and naive. use datetime.now(datetime.UTC) instead. Leave the file as it is for now; I will fix it myself. Reply OK." \
    && ok "session $n: correction" || ko "session $n: correction"
done

oncebitten evidence list --all
oncebitten evidence list --all | grep -q "user-correction .*sessions=2" && ok "cluster spans 2 sessions" || ko "cluster spans 2 sessions"

line=""
for _ in $(seq 1 $((WAIT_SECONDS / 5))); do
  line=$(oncebitten proposals list 2>"$ROOT/list.err" | head -1); [ -n "$line" ] && break
  grep -q "^triage failed" "$ROOT/list.err" && break; sleep 5
done
cat "$ROOT/list.err"; echo "$line"
id=${line%% *}
case "$line" in
  *" proposal "*" proven "*) ok "background triage queued a proven proposal" ;;
  *) ko "background triage queued a proven proposal"; oncebitten proposals show "$id" 2>&1 | head -20 ;;
esac
if [ -n "$id" ]; then
  oncebitten proposals show "$id"
  oncebitten proposals accept "$id" && ok "accept" || ko "accept"
  # The model writes the rule, so the number of existing hits varies by run.
  # The proof guarantees one thing: the rule matches its own bad fixture.
  rule=$(basename "$(dirname "$(ls .oncebitten/rules/*/rule.toml | head -1)")")
  bad=$(ls .oncebitten/rules/"$rule"/fixtures/bad.* | head -1)
  base=$(git rev-parse HEAD)
  oncebitten rules sync >/dev/null
  git add -A && commit accepted
  oncebitten gate pr --base "$base" && ok "gate passes existing code" || ko "gate passes existing code"
  cp "$bad" "src/new_case.${bad##*.}"
  git add -A && commit new
  oncebitten gate pr --base "$base"; [ $? = 1 ] && ok "gate fails a new copy of the bad fixture" || ko "gate fails a new copy of the bad fixture"
fi

# Steer: the shipped no-sudo rule must deny a real Bash call before it runs.
mkdir -p .oncebitten/rules && cp -r "$SRC/examples/no-sudo" .oncebitten/rules/
sed -i 's/^status = "proposed"/status = "active"/' .oncebitten/rules/no-sudo/rule.toml
oncebitten replay no-sudo >/dev/null && ok "no-sudo replays" || ko "no-sudo replays"
oncebitten hooks install --host claude --scope project >/dev/null
timeout 240 claude -p "Run exactly this shell command with the Bash tool and report what happened verbatim: sudo -n true" \
  --model haiku --setting-sources project,local --allowedTools Bash --output-format json >"$ROOT/steer.json" 2>>"$ROOT/claude.err"
grep -q "oncebitten no-sudo: Do not run sudo" "$ROOT/steer.json" && grep -q '"permission_denials":\[{' "$ROOT/steer.json" \
  && ok "steer hook denied sudo in a real session" || ko "steer hook denied sudo in a real session"

echo; echo "RESULT pass=$pass fail=$fail"
[ "$fail" = 0 ] || { echo "kept $ROOT for inspection"; exit 1; }
rm -rf "$ROOT"
