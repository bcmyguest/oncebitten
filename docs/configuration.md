# Configuration

Every setting has a key in a TOML file and an environment variable. The **Env variable** column names the variable. Keys marked *stored only* are parsed and validated; no command reads them yet.

## Precedence

1. Environment variables `ONCEBITTEN_*`.
2. Project file `.oncebitten/config.toml`. The loader checks the working directory, then each parent up to and including the git root (the first directory holding `.git`). Outside a git repository, no project file is read.
3. User file `$XDG_CONFIG_HOME/oncebitten/config.toml` when `XDG_CONFIG_HOME` is an absolute path, on every OS. Otherwise on Windows `%APPDATA%\oncebitten\config.toml`, and on Linux and macOS `~/.config/oncebitten/config.toml`.
4. Built-in defaults.

The loader merges per key. A key set in a higher layer replaces only that key. A `rules.sources` list replaces the whole list; its entries never merge across layers.

An empty environment variable counts as unset. `ONCEBITTEN_TRIGGERS_*` takes a comma-separated list.

Unknown keys and invalid values fail the load. The error names the file, or the environment layer for a bad variable. An unknown `ONCEBITTEN_*` variable, such as a misspelling, is ignored with a warning on stderr.

`HOME`, `APPDATA` and `XDG_CONFIG_HOME` count only when they hold an absolute path. A non-Unicode value in one of them, or in any `ONCEBITTEN_*` variable, fails the load; other non-Unicode variables are ignored.

`oncebitten config show --origin` prints the merged result and tags each key with its layer. `oncebitten config path` prints the user file and the project file it reads.

## Settings

| Key | Env variable | Default | Effect |
| --- | --- | --- | --- |
| `triage.default` | `ONCEBITTEN_TRIAGE_DEFAULT` | `claude:haiku` | Host and agent for triage. Stored only. |
| `triage.claude`, `triage.codex`, `triage.pi`, `triage.opencode` | `ONCEBITTEN_TRIAGE_CLAUDE`, `_CODEX`, `_PI`, `_OPENCODE` | unset | Triage target for that host. Stored only. The value must name the same host as the key. |
| `triggers.session` | `ONCEBITTEN_TRIGGERS_SESSION` | `user-correction`, `test-fail-then-pass`, `repeat-failure`, `revert` | Session signals that start triage. Stored only. Other values are rejected. |
| `triggers.gate` | `ONCEBITTEN_TRIGGERS_GATE` | `pre-push`, `pr-create` | Git points that run the gate. Stored only. Other values are rejected. |
| `sandbox.runtime` | `ONCEBITTEN_SANDBOX_RUNTIME` | `openshell` | Replay sandbox. The only accepted value is `openshell`. Stored only. |
| `rules.sources` | none (file only) | empty list | Rule repositories. Each entry has `name` (required; lowercase kebab-case, unique across entries), `url` (required), `writable` (default `false`) and `rev` (optional). At most one entry may set `writable = true`. Stored only. |

A triage target has the form `<host>:<agent>`. The host is `claude`, `codex`, `pi` or `opencode`. The agent is any non-empty string.

## Example

```toml
[triage]
default = "pi:default"

[triggers]
gate = ["pr-create"]

[[rules.sources]]
name = "team"
url = "https://github.com/example/team-rules.git"
writable = true
rev = "v1"
```
