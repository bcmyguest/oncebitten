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
- `src/main.rs`: CLI (`config show`, `config path`).
- `src/lib.rs`: library root.
- `tests/config.rs`: loader tests (precedence, merge, validation). `tests/cli.rs`: binary tests.
- `docs/configuration.md`: settings reference. Keep it in step with `src/config.rs`.
- `docs/research/`: written by other agents. Do not edit.

## Conventions

- Place constants before function and struct definitions in each file.
- Write no comments that explain what code does. Comment only a non-obvious why.
- Write the minimal code that solves the problem.
- Never special-case test inputs or hardcode expected values to pass a test.
- Tests use temp directories and pass `Inputs` with `HOME` and `XDG_CONFIG_HOME` set to temp paths. Never read the real `~/.config`.
- Commit messages: short Conventional Commits subject, trailer `Assisted-by: Claude:claude-haiku-5-5`, no `Co-Authored-By`.
