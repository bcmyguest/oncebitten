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

## Layout

- `src/config.rs`: layered configuration loader, validation, origin rendering.
- `src/rule.rs`: rule directory schema and loader (`rule.toml`, check file, fixtures).
- `src/registry.rs`: resolves project, source and example rules into qualified ids; precedence and overrides.
- `src/sources.rs`: source clone location and `git` invocations.
- `src/lockfile.rs`: `.oncebitten/rules.lock` read and write.
- `src/rules.rs`: `rules sync`, `rules list` and `rules validate` logic, including `--ci` checks.
- `src/proof.rs`: `proof.json` in a rule directory: directory hash, validity, writing a proof after an OpenShell replay.
- `src/gate.rs`: `gate pre-push|pr`: range of changed paths, proof checks, baseline filtering. Reuses `src/check.rs`.
- `src/baseline.rs`: `.oncebitten/baseline.json` entries, matching and writing. `gate::current_baseline` backs `baseline update`.
- `src/main.rs`: CLI (`config show`, `config path`, `rules sync|list|validate`, `check`, `replay`, `gate`, `baseline update`, `capture`, `hooks install|uninstall`).
- `src/lib.rs`: library root.
- `src/store.rs`: SQLite event store (`events`, `signals`).
- `src/signal.rs`: signal kinds and pure derivation from an event and its session history.
- `src/host.rs`: per-host hook payload to normalized event (claude, codex, cursor, copilot, opencode, pi).
- `src/capture.rs`: capture pipeline behind `oncebitten capture`.
- `src/hooks.rs`: installs and removes capture hooks for every host.
- `tests/config.rs`: loader tests (precedence, merge, validation). `tests/cli.rs`: binary tests.
- `tests/proof.rs`: proof hash, validity and `replay` CLI tests. `tests/gate.rs`: `gate` CLI tests. `tests/baseline_cli.rs`: `baseline update` tests. `tests/common/mod.rs`: shared temp git repo helper for the newer CLI tests.
- `tests/rule.rs`, `tests/registry.rs`: rule schema and resolution tests. `tests/sources.rs`: clone, sync and lockfile tests, using local git repos. `tests/rules_cli.rs`: `rules` subcommand tests.
- `docs/configuration.md`: settings reference. Keep it in step with `src/config.rs`.
- `docs/rules.md`: rule format, ids, overrides, lockfile and commands. Keep it in step with `src/rule.rs`, `src/registry.rs`, `src/rules.rs` and `src/proof.rs`.
- `tests/store.rs`, `tests/signal.rs`, `tests/host.rs`, `tests/hosts.rs`, `tests/capture.rs`, `tests/hooks.rs`, `tests/hooks_hosts.rs`, `tests/packaging.rs`, `tests/home.rs`: capture pipeline tests.
- `packaging/`: plugin manifests, hook files and the opencode and pi scripts. `tests/fixtures/`: real host payloads, scrubbed.
- `docs/gate.md`: gate stages, ranges, proofs, baseline, exit codes. Keep it in step with `src/gate.rs` and `src/baseline.rs`.
- `docs/capture.md`: signals, host support, hook install. Keep it in step with `src/signal.rs`, `src/host.rs` and `src/hooks.rs`.
- `docs/research/`: written by other agents. Do not edit.

## Conventions

- Place constants before function and struct definitions in each file.
- Write no comments that explain what code does. Comment only a non-obvious why.
- Write the minimal code that solves the problem.
- Never special-case test inputs or hardcode expected values to pass a test.
- Tests use temp directories and pass `Inputs` with `HOME` and `XDG_CONFIG_HOME` set to temp paths. Never read the real `~/.config`.
- Platforms: Linux, macOS and Windows are all supported. The binary contains no shell-specific code. Tests must pass on all three.
- Commit messages: short Conventional Commits subject, trailer `Assisted-by: Claude:claude-haiku-5-5`, no `Co-Authored-By`.
