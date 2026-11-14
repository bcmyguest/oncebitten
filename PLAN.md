# Plan v1: oncebitten

## Settled decisions
- Rules come from two sources: mined (same mistake class >=2 times) and stated (user says always/never; anger, frustration, bug reports count). Triage proposes any rule-shaped statement; user rejects one-offs in PR review.
- Rule modes: block (fails pre-push/CI), steer (guidance injected at the relevant moment), checklist (items verified at PR creation). Active once replay proof passes.
- Enforcement point is git (pre-push, PR creation, CI), not the agent: violations must not reach human review. Agent hooks only capture signals and steer.
- Signals are free/deterministic (exit codes, test fail->pass, repeated failure, revert, correction lexicon). Model triage fires on those events mid-session (auto-mode-like side channel) and at pre-push/PR.
- Triage model configurable `triage = "<host>:<agent>"` (claude:haiku, codex:<small>, pi:default, opencode:default) via headless CLI.
- Replay sandbox: NVIDIA OpenShell (bad version must fire, fixed must pass, valid corpus must pass).
- Windows: every replay requires WSL 2 plus OpenShell (no native Windows build as of v0.1.8). Without it, proposals ship unproven and inactive.
- Proposals delivered as a commit in the current work's PR, or a PR to the rules repo.
- Language: Rust single binary. ast-grep-core (crates.io 0.51.2, tree-sitter based) embedded for global cross-language rules; native linter (ruff/clippy/eslint) when the project already has one that fits and the rule is project-specific. OpenShell: openshell-sdk crate exists in NVIDIA/OpenShell but is not on crates.io -> shell out to `openshell` CLI first.
- Registry layers, precedence project > rules repos (ordered) > shipped examples. Project = .oncebitten/rules/. Rules repos = user/team-owned git repos listed in config `[rules] sources = [...]`, cloned locally, synced across machines/teammates; one marked writable receives approved rules. Tool ships EXAMPLE rules only (inactive) plus an llms.txt so an LLM can help a user adopt/adapt them into their rules repo.
- Config/docs modeled on smarter-compact.

## Gate guarantee
Pre-push is fast feedback only (bypassable with --no-verify). The guarantee is a required CI check (branch protection) before human review. Proposed rules stay inactive until merged; approval covers one exact rule version at one scope. Without a sandbox a proposal ships marked unproven and inactive. Gate exit codes separate "rule fired" from "check could not run". Lessons git cannot observe (e.g. running sudo) become steer rules or agent PreToolUse hooks.

## Steps (each ends with tests passing + user sign-off)
0. Scaffold: Rust crate, layered TOML config, docs skeleton modelled on smarter-compact.
1. Spikes (written up, code thrown away): OpenShell CLI replay of one ast-grep rule from Rust; ast-grep-core with Python/Rust/JS grammars incl. Result::unwrap; one headless JSON call per host.
2. Registry: per-rule TOML, `rules list|validate`, multi-repo resolution (repo-prefixed ids, explicit overrides, pinned revisions, writable routing, same result locally and in CI).
3. Block engine: ast-grep + native-linter passthrough; inactive example rules as bad/fixed fixtures; llms.txt.
4. Replay: fires on bad, passes on fixed, passes on valid corpus.
5. Gate: `gate pre-push|pr`, hook installer, required GitHub Action, PR-body checklist.
6. Signal capture: hooks for Claude/Codex/Cursor/opencode/pi, no model calls.
7. Evidence store + clustering: provenance, dedupe, independent occurrence counts, rejection memory.
8. Triage via host contract: hooks disabled in triage call, JSON output, timeout, bounded context.
9. Proposal lifecycle: same-PR commit or rules-repo PR; stale proofs re-replay.
10. Steer + checklist modes and their proofs; routing for lessons git cannot see.
11. Lifecycle: exceptions (reason + expiry), retirement, rollback, `oncebitten doctor`.

## Milestones and review
M1 = 0-1, M2 = 2-5, M3 = 6-9, M4 = 10-11. Haiku agents implement; Opus reviews each milestone; Codex gpt-6-astra reviews after Opus.
