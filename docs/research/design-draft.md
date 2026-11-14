# Draft: cross-agent "lesson loop" (failure -> proposed rule -> human gate -> enforcement)

## Goal
Agents (Claude Code, Codex, Cursor, opencode, pi) repeat mistakes. Detect failures and user corrections, classify them in a side channel
("repeatable?", "user correction?", "root cause?"), cluster into mistake classes, and propose the strongest enforcement that works.
Human decides general vs local. Preference: deterministic (architecture/types > lint > test/invariant > hook/harness) >>> markdown > nothing.

## Existing pieces (user's repos)
- smarter-compact: per-session SQLite turn store (messages, FTS5, vectors, events) for 6 hosts; one hook script normalizes hosts; plugin packaging for Claude/Codex/Cursor/Copilot.
- personal-memory (openmem): hook-harnesses.mjs per-host event table (sessionStart/prompt/stop) incl. Stop transcript parsing.
- skill-injector: `ski suggest` mines telemetry for repeats (>=2) and prints config changes, never edits. Same approval shape wanted.
- assisted-by: PreToolUse Bash guard; precedent of markdown rule (repeated in ~6 project memories) promoted to a hook.
- wteam adapters: headless invocations for claude/codex/cursor/opencode/pi (side-channel candidate).
- ~/.claude/projects/*/memory/*.md: existing manual lessons; many are hook candidates (no-sudo, no python3 -c heredocs, prefer Read over sed).

## External grounding
- Lauren Tan / pstack `correct` skill: mine commits/reverts/review comments; class counts after 2 occurrences; fix at highest layer
  (architecture, types, lint whose error names the fix, test, docs last); prove each check fails on a real past mistake; exceptions need reason+expiry+human.
- KNighter (arXiv 2503.09002): generated checker must flag buggy version and pass fixed one.
- HtT (2310.07064): keep rules by frequency + accuracy. Procedural memory (2606.23127): validate on held-out tasks; narrow skills overfit.
- AGENTS.md eval (2602.11988): context files don't raise success, +20% cost. IFScale (2507.11538): rule following decays with count.
- AgentSpec (2503.18666): runtime trigger/predicate/enforcement rules block >90% unsafe actions.
- Implicit feedback (2509.18361, 2507.23158): corrections detectable from follow-up prompts (13x explicit) but noisy -> propose, don't auto-accept.
- No primary research found on screenshots/diagrams raising PR acceptance.

## Pipeline
hosts' hooks -> turn store -> deterministic signal detectors (non-zero exit, test fail, revert, same file re-edited, user-correction phrases)
-> side-channel classifier (repeatable? correction? root cause? candidate layer) -> cluster by mistake class across sessions/repos (count >=2)
-> proposal (layer, artifact, scope, proof: replay fires on recorded bad action/diff, silent on fix) -> human gate (general / repo-local / reject / exception w/ expiry)
-> artifact lands (ast-grep/ruff/clippy/eslint rule, test, invariant, PreToolUse hook, harness change, AGENTS.md line last) -> rule->enforcer table.

## Round-1 open questions (to user)
1. Home: new repo vs extension of smarter-compact vs extension of ski.
2. Detection timing: live hooks vs batch mining (store + git history + PR comments) vs both.
3. Side-channel classifier: local model (Lemonade Qwen) vs headless claude -p / codex exec vs heuristics-only first.
4. Approval surface: proposals/ file queue + CLI vs GitHub issues/PRs in target repo vs in-session question.
5. Proof gate: replay mandatory for deterministic rules vs best effort.
6. PR evidence (screenshot/video/mermaid architecture+sequence diagram): enforce via PreToolUse on `gh pr create` vs skill/template only.
Deferred: rule artifact formats per language, global-rule distribution across agents, expiry policy, metrics.
