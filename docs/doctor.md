# Doctor

```sh
oncebitten doctor [--gateway <endpoint>]
```

`doctor` checks the installation and the repository the current directory is in. It writes nothing: it reads files, opens the evidence store read-only, and runs `pre-commit --version` and `openshell status`, each with a 10-second limit. It prints one line per check:

```text
ok    config: loads
fail  pre-commit: version 3.1.0 lacks the pre-push stage name (fix: install pre-commit 3.2.0 or later, e.g. pipx install pre-commit)
warn  capture codex: host found, capture hook not installed (fix: oncebitten hooks install --host codex --scope user)
```

Every `warn` and `fail` line ends with a fix. Exit 0 when no check fails, 1 when any check fails. Warnings never change the exit code.

## Checks

| Check | ok | warn | fail |
|---|---|---|---|
| `config` | the layered configuration loads | a configuration warning | the configuration does not load |
| `pre-commit` | `pre-commit --version` reports 3.2.0 or later | | missing, unreadable version, or older than 3.2.0 |
| `pre-push hook` | `.pre-commit-config.yaml` lists `oncebitten-gate` or `oncebitten-gate-system` | | no file, or neither hook id |
| `rules.lock` | every configured source has a lock entry with the same URL, and the lock names no other source | the file is missing | a source lacks an entry, an entry names an unknown source, or a URL differs |
| `rules` | every rule loads and resolves | | a rule error or a source that is not cloned |
| `proof <id>` | the proof of an active ast-grep or diff rule is valid | the proof of a `steer` rule is missing, stale or not proven | the proof of a `block` rule is missing, stale or not proven |
| `exceptions` | the file loads and no exception has expired | one line per expired exception | the file does not load |
| `capture <host>` | the capture hook is in the user or project file, or the host is not installed | the host is installed without the hook | |
| `evidence store` | the store opens, is absent (the first capture creates it), or has an older schema the next capture upgrades | | the file is not a readable store, or a newer oncebitten wrote it |
| `openshell` | `openshell status` exits 0 | the CLI is not on `PATH`, or the gateway does not answer | |

`rules.lock` and `rules` run only when the configuration loads. Outside a git repository, `doctor` prints one `repository` failure in place of the repository checks.

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
