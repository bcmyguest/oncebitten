# Plan v1: oncebitten

## Settled decisions
- Rules come from two sources: mined (same mistake class >=2 times) and stated (user says always/never; anger, frustration, bug reports count). Triage proposes any rule-shaped statement; user rejects one-offs in PR review.
- Rule modes: block (fails pre-push/CI) and steer (guidance injected at the relevant moment). Active once replay proof passes. No checklist mode: PR evidence (screenshot, diagram, test) is a block rule on the `diff` engine (`when_changed` globs require `require_changed` globs); items with no file to check become steer rules.
- Enforcement point is git (pre-push, PR creation, CI), not the agent: violations must not reach human review. Agent hooks only capture signals and steer.
- Signals are free/deterministic (exit codes, test fail->pass, repeated failure, revert, correction lexicon). Model triage fires on those events mid-session (auto-mode-like side channel) and at pre-push/PR.
- Triage model configurable `triage = "<host>:<agent>"` (claude:haiku, codex:<small>, pi:default, opencode:default) via headless CLI.
- Replay sandbox: NVIDIA OpenShell (bad version must fire, fixed must pass, valid corpus must pass).
- Windows: every replay requires WSL 2 plus OpenShell (no native Windows build as of v0.1.8). Without it, proposals ship unproven and inactive.
- Proposals delivered as a commit in the current work's PR, or a PR to the rules repo.
- Language: Rust single binary. ast-grep-core (crates.io 0.51.2, tree-sitter based) embedded for global cross-language rules; native linter (ruff/clippy/eslint) when the project already has one that fits and the rule is project-specific. OpenShell: openshell-sdk crate exists in NVIDIA/OpenShell but is not on crates.io -> shell out to `openshell` CLI first.
- Registry layers, precedence project > rules repos (ordered) > shipped examples. Project = .oncebitten/rules/. Rules repos = user/team-owned git repos listed in config `[rules] sources = [...]`, cloned locally, synced across machines/teammates; one marked writable receives approved rules. Tool ships EXAMPLE rules only (inactive) plus an llms.txt so an LLM can help a user adopt/adapt them into their rules repo.
- Config/docs modeled on smarter-compact.
- Rule layout: one directory per rule: `rule.toml`, check file (`check.yml` for ast-grep), `fixtures/bad.*`, `fixtures/fixed.*`.
- Rule ids: each `[[rules.sources]]` entry has a required `name`; ids are `<source-name>/<rule-id>`; project rules are bare ids.
- Overrides: a project rule replaces a source rule only with `overrides = "<source>/<id>"` and a `reason`; an unmarked id clash is an error.
- Reproducibility: committed `.oncebitten/rules.lock` pins each source to a commit; `rules sync` updates it; CI reads the lock and never fetches newer.
- Status: `status = "proposed" | "active" | "retired"` in `rule.toml`; approval flips it in the PR; CI verifies the replay proof for every active rule.

- Evidence store: SQLite (rusqlite, bundled), one database in the user data dir.
- Host hooks: installable at user or project scope (`--scope user|project`).
- Triage findings queue in the evidence store and surface at pre-push/PR, not mid-session.
- Git hooks ship as a `.pre-commit-hooks.yaml` entry; oncebitten writes no `.git/hooks` itself. The pre-commit framework is a hard requirement: README and install docs must say so up front.
- Forges: GitHub (`gh`), GitLab (`glab`), Azure DevOps (`az repos`), behind one forge trait.
- Existing violations: a committed baseline (`.oncebitten/baseline.json`) records violations present when a rule turns active; `check` and the gate fail only on violations not in the baseline.
- PR evidence (screenshot, video, diagram) may live as an attachment in the PR description or a PR comment, read through the forge trait (gh, glab, az) at PR time; a committed file also satisfies a `diff` rule.
- Correction lexicon: start broad and let triage filter; the list is configurable.
- Privacy: the event store keeps a SHA-256 of the prompt plus the matched phrase, never full prompt text.
- Triage writes the proposed rule and its fixtures in the same call.
- opencode is accepted as a triage host without a hooks-off proof; docs say so.
- pi support is verified against a locally installed pi.

## Gate guarantee
Pre-push is fast feedback only (bypassable with --no-verify). The guarantee is a required CI check (branch protection) before human review. Proposed rules stay inactive until merged; approval covers one exact rule version at one scope. Without a sandbox a proposal ships marked unproven and inactive. Gate exit codes separate "rule fired" from "check could not run". Lessons git cannot observe (e.g. running sudo) become steer rules or agent PreToolUse hooks.

## Steps (each ends with tests passing + user sign-off)
0. Scaffold: Rust crate, layered TOML config, docs skeleton modelled on smarter-compact.
1. Spikes (written up, code thrown away): OpenShell CLI replay of one ast-grep rule from Rust; ast-grep-core with Python/Rust/JS grammars incl. Result::unwrap; one headless JSON call per host.
2. Registry: per-rule TOML, `rules list|validate`, multi-repo resolution (repo-prefixed ids, explicit overrides, pinned revisions, writable routing, same result locally and in CI).
3. Block engine: ast-grep + native-linter passthrough + diff engine (`when_changed` -> `require_changed`); inactive example rules as bad/fixed fixtures; llms.txt. Rust `unwrap` ships as clippy `unwrap_used` passthrough: ast-grep has no types and does not see inside macros.
4. Replay: fires on bad, passes on fixed, passes on valid corpus. The real checker runs inside the sandbox and reports a structured result on stdout; exit codes alone never mean "fired". Separate tests for missing input, missing checker and sandbox/gateway failure, each classified as could-not-run. Checker: static musl binary on ast-grep crates, one JSON line, always exit 0; fired/clean only when JSON, path and exit 0 agree (openshell's own exit 1/2 collide with remote codes). OpenShell cannot mount host files: a read-only repo must be built into the image; upload leaves it writable (untested). Delete leftover sandboxes after a failed `create --upload` (it stays Ready).
5. Gate: `gate pre-push|pr`, `.pre-commit-hooks.yaml` entry, required CI check.
6. Signal capture: hooks for Claude/Codex/Cursor/opencode/pi, no model calls.
7. Evidence store + clustering: provenance, dedupe, independent occurrence counts, rejection memory.
8. Triage via host contract: hooks disabled in triage call, JSON output, timeout, bounded context. Run from an empty temp dir with `ONCEBITTEN_TRIAGE=1`, which every oncebitten hook checks and exits on. Prove hooks-off with sentinel hooks in every settings scope; refuse a host that fails the proof. Streaming hosts (opencode) need parse-then-kill. Settled: opencode is accepted without this proof, so the refusal does not apply to it.
9. Proposal lifecycle: same-PR commit or rules-repo PR; stale proofs re-replay.
10. Steer mode and its proofs; routing for lessons git cannot see.
11. Lifecycle: exceptions (reason + expiry), retirement, rollback, `oncebitten doctor`.

## Milestones and review
M1 = 0-1, M2 = 2-5, M3 = 6-9, M4 = 10-11. Haiku agents implement; Opus reviews each milestone; Codex gpt-6-astra reviews after Opus.
