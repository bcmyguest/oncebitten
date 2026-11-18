---
name: new-rule
description: Write a new oncebitten rule by hand, with fixtures and a replay proof. Use when the user states a rule to enforce ("never do X", "always do Y when Z changes"), asks to add or draft an oncebitten rule, or wants a mistake blocked at push or in the agent.
---

# New oncebitten rule

A rule is a directory `.oncebitten/rules/<id>/` in the repository. Field reference: <https://bcmyguest.github.io/oncebitten/rules/> and, for steer rules, <https://bcmyguest.github.io/oncebitten/steer/>. `oncebitten <command> --help` lists flags.

## 1. Pick the engine

State the mistake as one sentence, then take the first row that fits.

| The mistake is | Engine | Mode |
|---|---|---|
| a code pattern in one file (a call, an import, a cast) | `ast-grep` | `block` |
| "changed A without changing B" | `diff` | `block` |
| something only a linter, compiler or test can see | `native` | `block` |
| invisible to git: a command the agent runs, a file it must leave alone, a prompt | `steer` | `steer` |

Done when the user has confirmed the sentence and the engine.

## 2. Write the rule directory

`<id>` is lowercase kebab-case and equals the directory name. Write `status = "proposed"`.

```
.oncebitten/rules/<id>/
  rule.toml
  check.yml            # ast-grep only
  fixtures/bad.<ext>   # the mistake, minimal
  fixtures/fixed.<ext> # the same code done right
  corpus/              # look-alikes that must stay clean
```

```toml
id = "py-no-eval"
description = "Parse input with ast.literal_eval, not eval."
mode = "block"
status = "proposed"
engine = "ast-grep"
language = "python"
```

Per engine:

- `ast-grep`: `check.yml` is an ast-grep rule (`id`, `language`, `severity: error`, `message`, `rule`). Fixtures are source files. A `files:` glob must also match the fixture paths, so write `**/*.py`, not `src/**/*.py`.
- `diff`: add `when_changed` and `require_changed` glob arrays to `rule.toml`. Fixtures are `bad.diff` and `fixed.diff`: changed paths, one per line.
- `native`: add `command` (array), `fired_exit_codes`, and `fired_output_contains` when the tool uses the same exit code for a crash. It has no replay; fixtures are still required.
- `steer`: add `[steer]` with `message`, `action` (`inform`, or `deny` to refuse the tool call) and one `[steer.trigger]`: `tool` (+ `command` regex), `paths`, or `prompt` (with `at = "stop"`, a prompt rule delivers its message when the agent tries to finish the turn instead of with the prompt). Fixtures are `bad.json` and `fixed.json`, one normalized event each, such as `{"event": "tool", "tool": "Bash", "command": "sudo apt-get install jq"}`. Tool names differ per host; write the glob `{bash,shell,run_shell_command}`.

Put in `corpus/` at least one real file from this repository that resembles the mistake and is correct.

Done when `oncebitten rules validate` exits 0 and `oncebitten rules list` shows the id.

## 3. Prove it

```sh
oncebitten replay <id> --local
```

`proven` means the rule fired on `bad`, and stayed clean on `fixed` and every corpus file. On `rejected`, fix the pattern, not the fixtures, and replay again. Skip this step for `native` rules.

Done when the verdict is `proven`.

## 4. Activate on approval

Show the user `rule.toml`, the check and both fixtures. On approval:

1. Set `status = "active"`.
2. Run `oncebitten replay <id>` to write `proof.json`. Any later edit to the directory makes the proof stale, so this runs last. `ast-grep` and `diff` proofs need an OpenShell gateway; if the verdict is `could not run`, report the reason to the user and leave the rule active and unproven. `steer` proofs need no gateway. `native` rules need no proof.
3. Run `oncebitten check --base <default branch>` and report what the rule fires on in the existing code. For findings the user wants to keep, offer `oncebitten baseline update` or `oncebitten exception add`.

Done when the rule directory, with `proof.json` where one was written, is staged for the user to commit.
