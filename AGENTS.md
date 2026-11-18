# AGENTS.md

Rust crate `oncebitten`: a binary and a library, both named `oncebitten`. The build plan and settled decisions live in [PLAN.md](PLAN.md). Read it before changing structure.

## Commands

```sh
cargo build
cargo test
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

Run all four before a commit. CI runs the same commands.

Two scripts run a release binary end to end in a scratch repository. Neither runs in CI: both need an OpenShell gateway, and the second needs signed-in agent hosts.

```sh
cargo build --release
scripts/e2e.sh        # rules, replay, check, gate, exceptions, steer, capture, proposals, doctor; no agent host
scripts/e2e-live.sh   # real Claude Code sessions: capture, background triage, accept, gate, steer deny; spends model calls
```

## Layout

- `src/config.rs`: layered configuration loader, validation, origin rendering.
- `src/rule.rs`: rule directory schema and loader (`rule.toml`, check file, fixtures).
- `src/registry.rs`: resolves project, source and example rules into qualified ids; precedence and overrides.
- `src/sources.rs`: source clone location and `git` invocations.
- `src/lockfile.rs`: `.oncebitten/rules.lock` read and write.
- `src/rules.rs`: `rules sync`, `rules list` and `rules validate` logic, including `--ci` checks.
- `src/proof.rs`: `proof.json` in a rule directory: directory hash, validity, writing a proof after an OpenShell or matcher replay.
- `src/gate.rs`: `gate pre-push|pr`: range of changed paths, proof checks, baseline filtering. Reuses `src/check.rs`.
- `src/baseline.rs`: `.oncebitten/baseline.json` entries, matching and writing. `gate::current_baseline` backs `baseline update`.
- `src/exceptions.rs`: `.oncebitten/exceptions.toml`: parsing, expiry, matching findings, appending entries. Applied by `check`, `gate` and `baseline update`.
- `src/lifecycle.rs`: `rules retire` and `rules rollback`: in-place `rule.toml` status edits for project rules.
- `src/doctor.rs`: `oncebitten doctor` read-only checks. Reuses the config, lockfile, rule, proof, exception, hook and store loaders.
- `src/forge.rs`: `Forge` trait (find open PR, open PR, read PR text) with `gh`, `glab` and `az repos` backends, chosen from the `forge` key or the `origin` remote. `tests/support/fake_forge.rs`: the `fake-forge` test binary, copied as `gh`, `glab` or `az`. `tests/forge.rs` covers it. `tests/support/fake_host.rs`: the `fake-host` test binary, copied as `claude` (`claude.cmd` on Windows); `tests/triage_cli_host.rs` runs the CLI against it.
- `src/delivery.rs`: `proposals deliver` (PR commit or rules-repo branch). `tests/delivery.rs` covers it. `docs/proposals.md` has the flow.
- `src/attachments.rs`: pure matcher for attachment links in PR text; `gate pr` uses it for diff rules. `tests/gate_attachments.rs`.
- `src/main.rs`: CLI (`config show`, `config path`, `rules sync|list|validate|retire|rollback`, `check`, `replay`, `gate`, `baseline update`, `exception add|list`, `doctor`, `capture`, `hooks install|uninstall`, `evidence list|show|reject`).
- `src/lib.rs`: library root.
- `src/store.rs`: SQLite event store (`events`, `signals`, `rejections`, `proposals`, `decisions`, `steer_pending`). Events expire; see `docs/evidence.md`.
- `src/evidence.rs`: clusters signals into mistake classes, independent session counts, rejection memory. Backs `evidence list|show|reject`.
- `src/signal.rs`: signal kinds and pure derivation from an event and its session history.
- `src/host.rs`: per-host hook payload to normalized event (claude, codex, cursor, copilot, opencode, pi).
- `src/capture.rs`: capture pipeline behind `oncebitten capture`.
- `src/hooks.rs`: installs and removes capture and steer hooks for every host.
- `src/intent.rs`: experimental prompt-intent classifier, compiled only with the cargo feature `intent` and called by nothing in the binary. `bench/intent` and `docs/research/intent-classifier.md` hold its measurements; regex signals in `src/signal.rs` stay the capture path.
- `src/steer.rs`: steer rules: normalized event, trigger matcher, replay, per-host output, `oncebitten steer`, `at = "stop"` reminders held in the store until the host's stop event, and the session-start notice of waiting proposals. `src/host.rs` `normalize_steer` maps payloads onto its events and `normalize_boundary` maps session-start and stop payloads.
- `docs/steer.md`: steer rule format, fixtures, the steer hook and what each host supports. Keep it in step with `src/steer.rs`, `src/host.rs` and `src/hooks.rs`.
- `tests/steer.rs`, `tests/steer_gate.rs`, `tests/steer_hook.rs`, `tests/hooks_steer.rs`: steer schema, matcher, proof, gate, hook output and install tests.
- `tests/config.rs`: loader tests (precedence, merge, validation). `tests/cli.rs`: binary tests.
- `tests/proof.rs`: proof hash, validity and `replay` CLI tests. `tests/gate.rs`: `gate` CLI tests. `tests/baseline_cli.rs`: `baseline update` tests. `tests/common/mod.rs`: shared temp git repo helper for the newer CLI tests.
- `tests/rule.rs`, `tests/registry.rs`: rule schema and resolution tests. `tests/sources.rs`: clone, sync and lockfile tests, using local git repos. `tests/rules_cli.rs`: `rules` subcommand tests.
- `docs/configuration.md`: settings reference. Keep it in step with `src/config.rs`.
- `docs/rules.md`: rule format, ids, overrides, lockfile and commands. Keep it in step with `src/rule.rs`, `src/registry.rs`, `src/rules.rs` and `src/proof.rs`.
- `tests/store.rs`, `tests/signal.rs`, `tests/host.rs`, `tests/hosts.rs`, `tests/capture.rs`, `tests/hooks.rs`, `tests/hooks_hosts.rs`, `tests/hooks_scope.rs`, `tests/packaging.rs`, `tests/home.rs`: capture pipeline tests.
- `packaging/`: plugin manifests, hook files and the opencode and pi scripts. `packaging/<host>/bin/oncebitten` is the release launcher and `packaging/<host>/scripts/install.sh` a byte copy of `install.sh`, identical in every plugin; edit the claude copy and `install.sh`, then copy. `packaging/pi/package.js` is the pi package entry: everything from `const TIMEOUT_MS` on must match `packaging/pi/oncebitten.js`. `packaging/claude/skills/new-rule/SKILL.md` is the Claude Code skill for writing a rule by hand; keep it in step with `docs/rules.md` and `docs/steer.md`.
- Marketplaces: `.claude-plugin/`, `.agents/plugins/` (codex), `.github/plugin/` (copilot), `.cursor-plugin/` `marketplace.json`; `package.json` is the pi package manifest. `install.sh`, `install.ps1`: release installers. `.github/workflows/release.yml`: builds and publishes release binaries on a `v*` tag. `docs/install.md`: keep it in step with these. `tests/packaging.rs` covers versions, the marketplace and the installers. `tests/fixtures/`: real host payloads, scrubbed.
- `docs/gate.md`: gate stages, ranges, proofs, baseline, exceptions, exit codes. Keep it in step with `src/gate.rs` and `src/baseline.rs`.
- `docs/lifecycle.md`: exceptions, retire, rollback. Keep it in step with `src/exceptions.rs` and `src/lifecycle.rs`. `tests/exceptions.rs`, `tests/lifecycle.rs` cover them.
- `docs/doctor.md`: doctor checks and their levels. Keep it in step with `src/doctor.rs`. `tests/doctor.rs` covers it.
- `.pre-commit-hooks.yaml`: the `oncebitten-gate` pre-commit hook. `templates/`: CI gate jobs (GitHub Actions, GitLab CI, Azure Pipelines). `tests/gate.rs` checks both call real commands.
- `docs/capture.md`: signals, host support, hook install. Keep it in step with `src/signal.rs`, `src/host.rs` and `src/hooks.rs`.
- `docs/evidence.md`: clusters, counts, provenance, rejections. Keep it in step with `src/evidence.rs`. `tests/evidence.rs` covers library and CLI.
- `docs/roadmap.md`: milestones and step status. Keep it in step with PLAN.md "Steps" and "Milestones and review".
- `site/`: Starlight docs site built from `docs/*.md`. `site/scripts/lib/docs.mjs` `PAGES` lists every page and the sidebar; add a new doc there. `cd site && pnpm test && pnpm build` checks links. Diagrams: write the source as `docs/diagrams/<name>.mmd`, run `cd site && pnpm diagrams` to render `docs/media/<name>.svg` and `<name>-dark.svg` in the site theme, commit both, and embed them with the `<picture>` block from `docs/proposals.md`; `pnpm test` rejects ```` ```mermaid ```` fences in docs and unpaired renders. `.github/workflows/pages.yml` deploys it to https://bcmyguest.github.io/oncebitten/.
- `docs/research/`: written by other agents. Do not edit.

## Conventions

- Place constants before function and struct definitions in each file.
- Write no comments that explain what code does. Comment only a non-obvious why.
- Write the minimal code that solves the problem.
- Never special-case test inputs or hardcode expected values to pass a test.
- Tests use temp directories and pass `Inputs` with `HOME` and `XDG_CONFIG_HOME` set to temp paths. Never read the real `~/.config`.
- Platforms: Linux, macOS and Windows are all supported. The binary contains no shell-specific code. Tests must pass on all three.
- Commit messages: short Conventional Commits subject, trailer `Assisted-by: Claude:claude-haiku-5-5`, no `Co-Authored-By`.
