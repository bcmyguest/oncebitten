# Doctor

```sh
oncebitten doctor [--gateway <endpoint>]
```

`doctor` checks the installation and the repository the current directory is in. It writes nothing: it reads files, opens the evidence store read-only, and runs `pre-commit --version`, `prek --version` and `openshell status`, each with a 10-second limit. It reports missing tools with their install command and installs none of them. It prints one line per check:

```text
ok    config: loads
fail  pre-commit: version 3.1.0 lacks the pre-push stage name ; prek: No such file or directory (os error 2); uvx not on PATH (fix: install pre-commit 3.2.0 or later (pip install pre-commit; https://pre-commit.com/#install), or prek (uv tool install prek), or run it through uv with uvx prek install --hook-type pre-push (uv: https://docs.astral.sh/uv/getting-started/installation/))
warn  capture codex: host found, capture hook not installed (fix: oncebitten hooks install --host codex --scope user)
```

Every `warn` and `fail` line ends with a fix. Exit 0 when no check fails, 1 when any check fails. Warnings never change the exit code.

## Checks

| Check | ok | warn | fail |
|---|---|---|---|
| `config` | the layered configuration loads | a configuration warning | the configuration does not load |
| `pre-commit` | `pre-commit --version` reports 3.2.0 or later, else `prek --version` reports 0.1.5 or later, else `uvx` is on `PATH` | | none of the three holds |
| `pre-push hook` | `.pre-commit-config.yaml` lists `oncebitten-gate`, `oncebitten-gate-system` or `oncebitten-gate-python` | | no file, or none of the hook ids |
| `rules.lock` | every configured source has a lock entry with the same URL, and the lock names no other source | the file is missing | a source lacks an entry, an entry names an unknown source, or a URL differs |
| `rules` | every rule loads and resolves | | a rule error or a source that is not cloned |
| `proof <id>` | the proof of an active ast-grep or diff rule is valid | the proof of a non-`block` ast-grep or diff rule is missing, stale or not proven | the proof of a `block` or `steer` rule is missing, stale or not proven; `gate pr` exits 2 on the same rules |
| `exceptions` | the file loads and no exception has expired | one line per expired exception | the file does not load |
| `capture <host>` | the capture hook is in the user or project file, or the host is not installed | the host is installed without the hook | |
| `evidence store` | the store opens, is absent (the first capture creates it), or has an older schema the next capture upgrades | | the file is not a readable store, or a newer oncebitten wrote it |
| `openshell` | `openshell status` exits 0 | the CLI is not on `PATH`, or the gateway does not answer | |
| `<program>` | the first word of an active `native` rule's `command` is on `PATH` | it is not | |
| `docker` | `docker` is on `PATH`; checked only when a proof needs a replay | it is not | |
| `triage <host>` | the host program is on `PATH`; checked only for hosts that `triage.default` or a `triage.<host>` key names in a config layer or the environment | it is not | |

`rules.lock`, `rules`, the native-rule programs, `docker` and `triage <host>` run only when the configuration loads. A native command whose program contains a path separator, such as `./scripts/lint`, gets no check. Outside a git repository, `doctor` prints one `repository` failure in place of the repository checks.

A host counts as installed when its executable is on `PATH` or its user directory exists:

| Host | Executable | Directory |
|---|---|---|
| claude | `claude` | `~/.claude` |
| codex | `codex` | `~/.codex` |
| cursor | `cursor-agent` | `~/.cursor` |
| copilot | `copilot` | `~/.copilot` |
| opencode | `opencode` | `~/.config/opencode` |
| pi | `pi` | `~/.pi` |

`--gateway` passes `--gateway-endpoint <endpoint>` to `openshell status`; without it the CLI uses its own gateway. OpenShell runs on Windows only inside WSL 2, so on Windows the `openshell` fix says to install WSL 2 and run OpenShell and oncebitten inside it. Replay needs OpenShell; `check` and `gate` do not.

On Windows, `doctor` finds `pre-commit` and `openshell` through `PATH` and `PATHEXT`, so `.cmd` and `.exe` shims resolve.
