# oncebitten

oncebitten turns repeated agent mistakes into rules that git enforces. A mistake of the same class that recurs twice, or a rule the user states ("never push to main"), becomes a check that runs at pre-push, in CI and at PR creation, so a violation fails before human review. Prompt instructions bind an agent only when it reads them; a git gate binds every push.

Status: pre-alpha, step 5 of [PLAN.md](PLAN.md). Rules load from three layers, `oncebitten check` runs them, `oncebitten replay` proves them in a sandbox and `oncebitten gate` blocks pushes and pull requests that add violations.

The pre-push gate requires the [pre-commit](https://pre-commit.com) framework (3.2.0 or later): oncebitten ships a pre-commit hook, not a git hook of its own. CI runs `oncebitten gate pr` directly. See [docs/gate.md](docs/gate.md).

## Install from source

```sh
git clone https://github.com/bcmyguest/oncebitten
cd oncebitten
cargo install --path .
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
oncebitten hooks install --host claude --scope user   # also codex, cursor, copilot, opencode, pi
oncebitten hooks uninstall --host claude --scope user
```

Rules: [docs/rules.md](docs/rules.md). Gate, proofs and baseline: [docs/gate.md](docs/gate.md). Agent-facing summary: [llms.txt](llms.txt).

Hooks record prompts and tool outcomes locally: [docs/capture.md](docs/capture.md) lists what each host reports and how each was verified.

Settings and their precedence: [docs/configuration.md](docs/configuration.md).

## Gate

```yaml
# .pre-commit-config.yaml
repos:
  - repo: https://github.com/bcmyguest/oncebitten
    rev: <commit or tag>
    hooks:
      - id: oncebitten-gate
```

```sh
pre-commit install --hook-type pre-push
oncebitten rules sync             # writes .oncebitten/rules.lock; commit it
```

CI templates for GitHub Actions, GitLab CI and Azure Pipelines: [templates/](templates/).

## License

Apache-2.0. See [LICENSE](LICENSE).
