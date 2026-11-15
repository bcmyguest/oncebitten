# Rules

A rule is a directory. Rules come from three layers, and the project layer wins.

## Rule directory

```
<rule-id>/
  rule.toml
  check.yml          # ast-grep only
  fixtures/
    bad.<ext>        # exactly one
    fixed.<ext>      # exactly one; diff rules use bad.diff and fixed.diff
```

`id` must equal the directory name.

Directories whose names start with `.` are skipped, so a source clone's `.git` is never a rule.

`rule.toml` fields:

| Field | Required | Values |
|---|---|---|
| `id` | yes | lowercase kebab-case, equal to the directory name |
| `description` | yes | text |
| `mode` | yes | `block`, `steer` |
| `status` | yes | `proposed`, `active`, `retired` |
| `engine` | yes | `ast-grep`, `native`, `diff` |
| `language` | no | text |
| `overrides` | no | `<source>/<id>`; requires `reason` |
| `reason` | with `overrides` | text |
| `command` | `native` only | non-empty array of strings |
| `fired_exit_codes` | `native` only, optional | array of non-zero integers; default empty |
| `fired_output_contains` | `native` only, with `fired_exit_codes` | non-empty text; a substring the output must contain |
| `when_changed` | `diff` only | non-empty array of glob strings |
| `require_changed` | `diff` only | non-empty array of glob strings |

Engines:

- `ast-grep`: needs `check.yml`.
- `native`: runs `command` in the repository root. Takes no check file. Exit 0 is clean. An exit code listed in `fired_exit_codes` is a fired finding, and its output (stderr, else stdout) is the message. Any other exit code, a missing binary, or termination by signal is could-not-run. With no `fired_exit_codes`, the rule never fires.
  With `fired_output_contains`, a listed exit code fires only when a line of stdout or stderr contains the text. Without such a line, the run is could-not-run. A matched line that is a JSON object with `message.rendered` (cargo `--message-format=json`) is reported as that rendered text; other matched lines are reported as they are. Rules without the key keep the exit-code-only behaviour.
- `diff`: takes no check file. Its fixtures list changed paths, one per line, relative to the repository root with `/` separators. `check` passes the paths from `git diff --name-only <base>` plus untracked files. It fires when a `when_changed` glob matches a changed path and no `require_changed` glob matches any changed path. The finding names the matching paths and the required globs. Globs use `globset` syntax with `literal_separator`: `*` stays within one directory, `**` crosses directories. An invalid glob is could-not-run.

Unknown keys fail the load. Every error names the file.

## Ids and layers

| Layer | Location | Qualified id |
|---|---|---|
| project | `<repo>/.oncebitten/rules/<id>/` | `<id>` |
| source | `[[rules.sources]]` clone, `<name>` in config | `<source>/<id>` |
| examples | shipped in the binary (`examples/` in the repository), only those named in `rules.use_examples` | `examples/<id>` |

Precedence: project, then sources in config order, then examples.

Examples are opt-in. Without `rules.use_examples`, no example is loaded, extracted or shown by `check`, `rules list` or `rules validate`. Each id in `rules.use_examples` must name a shipped example directory, and that rule is then loaded as active. Shipped examples have `status = "proposed"` in their file; listing one overrides that. To change a listed example, copy it into `.oncebitten/rules/` under an id of its own.

```toml
[rules]
use_examples = ["py-no-print", "rust-no-unwrap"]
```

Shipped examples:

| Id | Engine | Fires on |
|---|---|---|
| `py-utcnow` | ast-grep (python) | `datetime.utcnow()` |
| `py-no-print` | ast-grep (python) | `print(...)` |
| `js-moment` | ast-grep (javascript) | importing or requiring `moment` |
| `ts-no-as-any` | ast-grep (typescript) | `x as any` |
| `rust-no-unwrap` | native, `cargo clippy -D clippy::unwrap_used` | `.unwrap()` in a crate (exit 101 with the `clippy::unwrap_used` diagnostic is fired; a crate that does not compile is could-not-run) |
| `arch-diagram` | diff | `src/**/mod.rs` or `src/lib.rs` changed without `docs/architecture/*.svg` or `*.png` |
| `ui-screenshot` | diff | `src/ui/**` changed without `docs/screenshots/*.png` |

Native rules fire on exit 101, which cargo also returns for a compile error. The `rust-no-unwrap` fixtures are whole crate roots (`src/main.rs`) for the rule's own tests. `check` runs the rule in the repository root.

A project rule with a bare id equal to a source rule's id is an error unless it sets `overrides`. `overrides` must name an existing id. A project rule with `overrides = "team/x"` replaces `team/x`. Two sources defining the same qualified id is an error.

## Sources and the lockfile

Each `[[rules.sources]]` entry needs a `name` (kebab-case, unique) and a `url`. An optional `rev` names the branch, tag or commit to check out.

`rules sync` clones each source to `$XDG_DATA_HOME/oncebitten/sources/<name>` when `XDG_DATA_HOME` is an absolute path. Otherwise it uses `~/.local/share/oncebitten/sources/<name>` on Unix and `%LOCALAPPDATA%\oncebitten\sources\<name>` on Windows. It then writes `<repo>/.oncebitten/rules.lock`:

```toml
[[source]]
name = "team"
url = "https://example.invalid/team-rules.git"
commit = "<full 40-character SHA>"
```

Git runs through the `git` binary.

## Commands

| Command | Does | Exit |
|---|---|---|
| `oncebitten rules sync` | Clone or fetch each source, check out `rev` or `origin/HEAD`, write `rules.lock` | 0 on success |
| `oncebitten rules list [--ci]` | Print `id  mode  status  origin` per rule | 0; 1 on rule errors; 2 when the check cannot run |
| `oncebitten rules validate [--ci]` | Load and resolve every rule | 0 if clean; 1 on rule errors; 2 when the check cannot run |
| `oncebitten check [--base <rev>]` | Run each `active` rule against changes since `<rev>` (default `HEAD`), print each finding as `<id>: <path>[:<line>]: <message>` (no path: `<id>: <message>`) | 0 clean; 1 any rule fired; 2 any rule could not run, or the rules did not load (2 wins over 1) |

`check` prints a could-not-run reason to stderr as `<id>: could not run: <reason>`. It reads the repository the current directory is in.

`--ci` never fetches. It requires each source clone to sit at the commit in `rules.lock` (`git rev-parse HEAD`). A missing lockfile, a missing entry, or a mismatch exits 2.

Without `--ci`, a source that is not cloned exits 2 and asks for `rules sync`.
