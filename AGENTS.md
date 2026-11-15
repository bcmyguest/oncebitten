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
- `src/main.rs`: CLI (`config show`, `config path`, `capture`, `hooks install|uninstall`).
- `src/lib.rs`: library root.
- `src/store.rs`: SQLite event store (`events`, `signals`).
- `src/signal.rs`: signal kinds and pure derivation from an event and its session history.
- `src/host.rs`: per-host hook payload to normalized event (claude, codex, cursor, copilot, opencode, pi).
- `src/capture.rs`: capture pipeline behind `oncebitten capture`.
- `src/hooks.rs`: installs and removes capture hooks for every host.
- `tests/config.rs`: loader tests (precedence, merge, validation). `tests/cli.rs`: binary tests.
- `tests/store.rs`, `tests/signal.rs`, `tests/host.rs`, `tests/hosts.rs`, `tests/capture.rs`, `tests/hooks.rs`, `tests/hooks_hosts.rs`, `tests/packaging.rs`, `tests/home.rs`: capture pipeline tests.
- `docs/configuration.md`: settings reference. Keep it in step with `src/config.rs`.
- `packaging/`: plugin manifests, hook files and the opencode and pi scripts. `tests/fixtures/`: real host payloads, scrubbed.
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
