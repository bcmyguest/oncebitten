# <img src="docs/media/icon.svg" alt="" width="36" align="absmiddle"> oncebitten

**oncebitten turns repeated agent mistakes into rules that git enforces.** When the same mistake shows up in two sessions, or you state a rule once, it proposes a check. You approve it, and the check then runs at pre-push and in CI.

<p align="center">
<picture>
<source media="(prefers-color-scheme: dark)" srcset="docs/media/oncebitten-dark.svg" />
<img src="docs/media/oncebitten.svg" alt="Caught: a Claude correction, a Codex revert and a failing Cursor test each count as evidence. Rule: oncebitten proposes the strongest rule that fits (a check, then an instruction, then nothing) and you approve it. Enforced: git push, CI and pull requests stop on py-utcnow: use datetime.now(UTC)." width="720" />
</picture>
</p>

Docs: https://bcmyguest.github.io/oncebitten/

Status: pre-alpha. Capture, evidence clustering, `check`, `replay`, the gate, steer rules, exceptions, retire, rollback and `doctor` work; triage is implemented for claude, codex and opencode ([docs/triage.md](docs/triage.md)): the claude path runs end to end against real Claude Code sessions (`scripts/e2e-live.sh`), codex 0.167.2 fails the hooks-off proof and is refused, and opencode has fake-host tests only; triage drafts proposed rules that wait for review in `oncebitten proposals` ([docs/proposals.md](docs/proposals.md)) and `proposals deliver` sends an accepted rule to a pull request.

## Install

Pick the host you use; each section is copy-pasteable. The plugins download the release binary on first use and check its SHA-256. For the command line (gate, `evidence`, `check`) see [Command line](#command-line).

### Claude Code

```
/plugin marketplace add bcmyguest/oncebitten
/plugin install oncebitten@oncebitten
```

### Codex

```sh
codex plugin marketplace add bcmyguest/oncebitten
```

Then install `oncebitten` from `/plugins`.

### Copilot CLI

```sh
copilot plugin marketplace add bcmyguest/oncebitten
copilot plugin install oncebitten@oncebitten
```

### Cursor

Import `https://github.com/bcmyguest/oncebitten` as a team marketplace (Dashboard, Settings, Plugins).

### pi

```sh
pi install git:github.com/bcmyguest/oncebitten
```

### opencode

opencode loads plugins from npm or local files only, so install the command line below, then:

```sh
oncebitten hooks install --host opencode --scope user
```

Do not run `hooks install` for a host whose plugin is installed: each event would be captured twice.

### Command line

Any one of these puts `oncebitten` and `oncebitten-checker` on your `PATH`. None needs a Rust toolchain except the last.

```sh
uvx oncebitten --version               # run once, no install (PyPI)
uv tool install oncebitten             # install (PyPI)
npx oncebitten --version               # run once (npm)
npm install -g oncebitten              # install (npm)
curl -fsSL https://raw.githubusercontent.com/bcmyguest/oncebitten/main/install.sh | sh   # macOS, Linux
irm https://raw.githubusercontent.com/bcmyguest/oncebitten/main/install.ps1 | iex        # Windows PowerShell
cargo install --locked --git https://github.com/bcmyguest/oncebitten oncebitten          # from source; needs a C compiler
```

`install.sh` and `install.ps1` verify the release's SHA-256 before installing. Pinning, mirrors and variables: [docs/install.md](docs/install.md).

### Teams

Pin one version for everyone so the gate behaves the same on every machine and in CI.

```toml
# pyproject.toml
[dependency-groups]
dev = ["oncebitten==0.2.0"]
```

```json
{ "devDependencies": { "oncebitten": "0.2.0" } }
```

```yaml
# .pre-commit-config.yaml: prebuilt binary from PyPI, no Rust toolchain
repos:
  - repo: https://github.com/bcmyguest/oncebitten
    rev: v0.2.0
    hooks:
      - id: oncebitten-gate-python
```

## Usage

```sh
oncebitten config show            # merged configuration as TOML
oncebitten config show --origin   # annotate each value with its layer
oncebitten config path            # configuration files it reads
oncebitten check                  # run active rules against changes since HEAD
oncebitten replay <rule-id>       # prove a rule in an OpenShell sandbox; writes proof.json
oncebitten gate pr --base origin/main   # CI gate: new violations of active block rules
oncebitten baseline update        # accept today's violations in .oncebitten/baseline.json
oncebitten hooks install --host claude --scope user   # capture and steer hooks; also codex, cursor, copilot, opencode, pi
oncebitten hooks uninstall --host claude --scope user
oncebitten evidence list          # mistake clusters seen in 2+ sessions, or stated once
oncebitten evidence reject <id> --reason "one-off"
oncebitten stats [--since <days>] [--json]   # read-only funnel: signals to clusters to drafts to decisions
```

Rules: [docs/rules.md](docs/rules.md). Example rules: [examples/](examples/README.md). Gate, proofs and baseline: [docs/gate.md](docs/gate.md). Agent-facing summary: [llms.txt](llms.txt).

Funnel counts per mistake class: [docs/stats.md](docs/stats.md).

Hooks record prompts and tool outcomes locally: [docs/capture.md](docs/capture.md) lists what each host reports and how each was verified. [docs/evidence.md](docs/evidence.md) covers clustering and rejections.

Steer rules cover lessons git cannot see, such as running `sudo`: they inform or deny the agent at the tool call. [docs/steer.md](docs/steer.md) lists what each host supports.

Settings and their precedence: [docs/configuration.md](docs/configuration.md).

## Gate

```yaml
# .pre-commit-config.yaml
repos:
  - repo: https://github.com/bcmyguest/oncebitten
    rev: <commit or tag>
    hooks:
      - id: oncebitten-gate-python   # prebuilt wheel; or oncebitten-gate (builds from source), oncebitten-gate-system (installed binary)
```

```sh
pre-commit install --hook-type pre-push
oncebitten rules sync             # writes .oncebitten/rules.lock; commit it
```

The gate needs [pre-commit](https://pre-commit.com) 3.2.0 or later, [prek](https://github.com/j178/prek) 0.1.5 or later, or [uv](https://docs.astral.sh/uv/)'s `uvx` to run either. CI templates for GitHub Actions, GitLab CI and Azure Pipelines: [templates/](templates/). In CI, run `oncebitten rules sync --frozen` before the gate.

## Development

Build and test with `cargo test`; CI also runs `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` on Linux, macOS and Windows. The build order and open steps live in [PLAN.md](PLAN.md).

This repository gates itself with the shipped `rust-no-unwrap` example, enabled in [`.oncebitten/config.toml`](.oncebitten/config.toml). It has no baseline: the gate fails on any non-test `.unwrap()` in the crate, and test code is exempt.

## License

Apache-2.0. See [LICENSE](LICENSE).
