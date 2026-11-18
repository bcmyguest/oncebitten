# Configuration

Every setting has a key in a TOML file and an environment variable, except `forge` and the two `capture` lists, which are TOML only. The **Env variable** column names the variable. Keys marked *stored only* are parsed and validated; no command reads them yet.

## Precedence

1. Environment variables `ONCEBITTEN_*`.
2. Project file `.oncebitten/config.toml`. The loader checks the working directory, then each parent up to and including the git root (the first directory holding `.git`). Outside a git repository, no project file is read.
3. User file `$XDG_CONFIG_HOME/oncebitten/config.toml` when `XDG_CONFIG_HOME` is an absolute path, on every OS. Otherwise on Windows `%APPDATA%\oncebitten\config.toml`, and on Linux and macOS `~/.config/oncebitten/config.toml`.
4. Built-in defaults.

The loader merges per key. A key set in a higher layer replaces only that key. A `rules.sources` list replaces the whole list; its entries never merge across layers.

An empty environment variable counts as unset. `ONCEBITTEN_TRIGGERS_*` takes a comma-separated list.

Unknown keys and invalid values fail the load. The error names the file, or the environment layer for a bad variable. An unknown `ONCEBITTEN_*` variable, such as a misspelling, is ignored with a warning on stderr.

`HOME`, `USERPROFILE`, `HOMEDRIVE`, `HOMEPATH`, `APPDATA` and `XDG_CONFIG_HOME` count only when they hold an absolute path. A non-Unicode value in one of them, or in any `ONCEBITTEN_*` variable, fails the load; other non-Unicode variables are ignored.

The home directory is `HOME`. On Windows, which sets no `HOME` by default, it falls back to `USERPROFILE`, then to `HOMEDRIVE` plus `HOMEPATH`. `oncebitten hooks` uses it for user scope. On Windows the user config lives under `APPDATA` and the event database under `LOCALAPPDATA`; when either is unset or relative, the home directory's `AppData\Roaming` or `AppData\Local` is used instead.

`oncebitten config show --origin` prints the merged result and tags each key with its layer. `oncebitten config path` prints the user file and the project file it reads.

## Settings

| Key | Env variable | Default | Effect |
| --- | --- | --- | --- |
| `forge` | none (TOML only) | unset | Forge CLI for pull requests: `github` (`gh`), `gitlab` (`glab`) or `azure` (`az repos`). Unset, oncebitten picks one from the `origin` remote host: `github.com` is GitHub; `gitlab.com` or a host containing `gitlab` is GitLab; `dev.azure.com` or `*.visualstudio.com` is Azure DevOps. An unknown host with no `forge` is an error that names this key. Other values are rejected. |
| `triage.default` | `ONCEBITTEN_TRIAGE_DEFAULT` | `claude:haiku` | Host and agent for triage. |
| `triage.claude`, `triage.codex`, `triage.pi`, `triage.opencode` | `ONCEBITTEN_TRIAGE_CLAUDE`, `_CODEX`, `_PI`, `_OPENCODE` | unset | Triage target for that host. The value must name the same host as the key. Background triage asks the capturing host through this target when set. |
| `triage.send_code` | none (TOML only) | `true` | Sends the code hunks that a session changed to the triage host, and requires an ast-grep or native draft's `bad` fixture to quote a consecutive run of one hunk's post-image that includes a substantive (not blank, not lone-brace) added line. Diff drafts are exempt. Off keeps the prompt without code. Code hunks go to the configured triage host, and the user is responsible for that host's data handling. See [triage.md](triage.md#code-hunks). |
| `triage.max_code_bytes` | none (TOML only) | `16384` | Byte cap on the code hunks in a triage prompt. Must be at least 1 while `send_code` is on. |
| `triage.background` | none (TOML only) | `true` | `capture` starts triage in the background when a cluster reaches its threshold. See [proposals.md](proposals.md#background-triage). |
| `triage.max_per_day` | none (TOML only) | `10` | Triage runs per local calendar day. Each background run spends one; `capture` records `skipped: budget` and starts nothing once the day's runs reach the cap. Manual `triage <id>` still runs and warns on stderr. Must be a whole number; `0` skips every background run. See [triage.md](triage.md#daily-budget). |
| `triggers.session` | `ONCEBITTEN_TRIGGERS_SESSION` | `user-correction`, `test-fail-then-pass`, `repeat-failure`, `revert` | Session signals that start background triage. A cluster whose class has no entry here starts nothing; the cluster stays captured. `tool-failure` clusters use the `repeat-failure` entry. An empty list turns background triage off. Other values are rejected. |
| `capture.correction_phrases` | none (TOML only) | the built-in list in `src/signal.rs` | Replaces the correction lexicon that `user-correction` matches. Must list at least one phrase. Blank phrases fail the load. |
| `capture.extra_correction_phrases` | none (TOML only) | `[]` | Adds phrases to the lexicon in effect. Same validation as above. |
| `capture.test_commands` | none (TOML only) | `[]` | Command prefixes that count as test runs for `test-fail-then-pass`, next to `cargo test`, `pytest`, `npm test`, `vitest`, `jest` and `go test`. Blank entries fail the load. |
| `evidence.ttl_days` | none (TOML only) | `30` | Days a captured event lives. `capture` deletes older events unless a waiting proposal names them. Must be at least 1. Evidence is temporary; see [evidence.md](evidence.md). |
| `triggers.gate` | `ONCEBITTEN_TRIGGERS_GATE` | `pre-push`, `pr-create` | Git points that run the gate. Stored only: `oncebitten gate` runs wherever its pre-commit hook or CI job is installed (see [gate.md](gate.md)). Other values are rejected. |
| `sandbox.runtime` | `ONCEBITTEN_SANDBOX_RUNTIME` | `openshell` | Replay sandbox. The only accepted value is `openshell`. Stored only. |
| `rules.use_examples` | none (file only) | empty list | Shipped example rules to load, by directory name, which equals the rule id (e.g. `rust-no-unwrap`). Each listed example is active. An unknown name is an error. Like other lists, a higher layer replaces the whole list. |
| `rules.sources` | none (file only) | empty list | Rule repositories. Each entry has `name` (required; lowercase kebab-case, unique across entries; `local` is reserved for the local rule layer), `url` (required), `writable` (default `false`) and `rev` (optional). At most one entry may set `writable = true`. Stored only. |

A triage target has the form `<host>:<agent>`. The host is `claude`, `codex`, `pi` or `opencode`. The agent is any non-empty string.

## Example

```toml
forge = "github"

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
