# Rules

A rule is a directory. Rules come from three layers, and the project layer wins.

## Rule directory

```
<rule-id>/
  rule.toml
  check.yml          # ast-grep only
  fixtures/
    bad.<ext>        # exactly one
    fixed.<ext>      # exactly one; diff rules use bad.diff and fixed.diff, steer rules bad.json and fixed.json
  corpus/            # replay only: files that must stay clean
  proof.json         # written by a proven OpenShell or matcher replay; not part of the hash
```

`id` must equal the directory name.

Directories whose names start with `.` are skipped, so a source clone's `.git` is never a rule.

`rule.toml` fields:

| Field | Required | Values |
|---|---|---|
| `id` | yes | lowercase kebab-case, equal to the directory name |
| `description` | yes | text |
| `mode` | yes | `block`, `steer`. `block` rules run in `check` and `gate`; `steer` rules run only in agent hooks (see [steer.md](steer.md)). |
| `status` | yes | `proposed`, `active`, `retired`; only `active` runs (see [lifecycle.md](lifecycle.md)) |
| `retired_reason` | no | non-empty text; `rules retire` writes it |
| `engine` | yes | `ast-grep`, `native`, `diff`, `steer` (only with `mode = "steer"`) |
| `language` | no | text |
| `overrides` | no | `<source>/<id>`; requires `reason` |
| `reason` | with `overrides` | text |
| `command` | `native` only | non-empty array of strings |
| `fired_exit_codes` | `native` only, optional | array of non-zero integers; default empty |
| `fired_output_contains` | `native` only, with `fired_exit_codes` | non-empty text; a substring the output must contain |
| `when_changed` | `diff` only | non-empty array of glob strings |
| `require_changed` | `diff` only | non-empty array of glob strings |
| `[steer]` | `steer` only | `action`, `message` and a `[steer.trigger]` table; see [steer.md](steer.md) |

Engines:

- `ast-grep`: needs `check.yml`. `files:` and `ignores:` globs match the repo-relative `/`-separated path in `check`, `gate` and `oncebitten-checker`. In replay the checker receives the fixture path, so a scoped rule's globs must match its fixtures (for example `**/*.py`). `severity: off` drops the rule in every path.
- `native`: runs `command` in the repository root. Takes no check file. Exit 0 is clean. An exit code listed in `fired_exit_codes` is a fired finding, and its output (stderr, else stdout) is the message. Any other exit code, a missing binary, or termination by signal is could-not-run. With no `fired_exit_codes`, the rule never fires.
  With `fired_output_contains`, a listed exit code fires only when a line of stdout or stderr contains the text. Without such a line, the run is could-not-run. A matched line that is a JSON object with `message.rendered` (cargo `--message-format=json`) is reported as that rendered text; other matched lines are reported as they are. Rules without the key keep the exit-code-only behaviour.
- `steer`: takes no check file and needs `mode = "steer"`. Its fixtures are recorded hook events in JSON. `check` and `gate` skip it; the `oncebitten steer` agent hook runs it. See [steer.md](steer.md).
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

Native rules fire only when the command exits with a code in `fired_exit_codes` and its output contains `fired_output_contains`, if set. `rust-no-unwrap` sets both: it fires on exit 101 with the `"code":"clippy::unwrap_used"` JSON diagnostic. Cargo also exits 101 on a compile error, so that case is could-not-run. The `rust-no-unwrap` fixtures are whole crate roots (`src/main.rs`) for the rule's own tests. `check` runs the rule in the repository root.

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
| `oncebitten rules sync --frozen` | Clone any missing source, check out each locked commit, fetching one only when the clone lacks it; never writes `rules.lock` | 0 on success; 2 when the lock is missing, a source has no entry, the lock names a source the config no longer has, or a locked commit is unreachable after fetching |
| `oncebitten rules list [--ci]` | Print `id  mode  status  origin` per rule | 0; 1 on rule errors; 2 when the check cannot run |
| `oncebitten rules validate [--ci]` | Load and resolve every rule | 0 if clean; 1 on rule errors; 2 when the check cannot run |
| `oncebitten rules retire <id> --reason <text>` | Set `status = "retired"` and `retired_reason` on a project rule (see [lifecycle.md](lifecycle.md#retire)) | 0 retired; 2 unknown, source or example rule, empty reason, or already retired |
| `oncebitten rules rollback <id>` | Return an active project rule to `proposed`, keep its files and proof, print the git commands that restore an earlier version (see [lifecycle.md](lifecycle.md#rollback)) | 0 rolled back; 2 unknown, source or example rule, or not active |
| `oncebitten exception add\|list` | Add or list exceptions in `.oncebitten/exceptions.toml` (see [lifecycle.md](lifecycle.md#exceptions)) | 0; 2 invalid entry or file |
| `oncebitten doctor [--gateway <endpoint>]` | Read-only checks of the installation and the repository (see [doctor.md](doctor.md)) | 0 no check failed; 1 a check failed |
| `oncebitten replay --stale [--checker <path>] [--gateway <endpoint>] [--local]` | Replay every project rule with status `active` or `proposed` whose proof is missing or stale; print `<id>: proven`, `<id>: rejected: <reason>` or `<id>: could not run: <reason>` per rule | 0 all proven or none needed; 1 any rejected; 2 any could not run (2 wins) |
| `oncebitten replay <rule-id> [--checker <path>] [--gateway <endpoint>] [--local]` | Replay one ast-grep or diff rule and write `proof.json` when an OpenShell replay proves it; replay a steer rule with the matcher and write `proof.json` when it proves it (see [Replay](#replay)) | 0 proven; 1 rejected; 2 could not run |
| `oncebitten steer --host <host>` | Agent hook: match the payload on stdin against active steer rules and print the host's output (see [steer.md](steer.md)) | always 0 |
| `oncebitten gate pre-push [--base <ref>]`, `oncebitten gate pr --base <ref>` | Run active block rules on the changed paths, require proofs (for active steer rules too), apply the baseline (see [gate.md](gate.md)) | 0 clean; 1 a new violation; 2 could not run, or (`pr`) a proof is missing or stale |
| `oncebitten baseline update` | Replace `.oncebitten/baseline.json` with the current violations of active block rules in every tracked file | 0 written; 2 a rule could not run (nothing written) |
| `oncebitten check [--base <rev>]` | Run each `active` block rule against changes since `<rev>` (default `HEAD`), drop findings a live exception covers, print each finding as `<id>: <path>[:<line>]: <message>` (no path: `<id>: <message>`) | 0 clean; 1 any rule fired; 2 any rule could not run, or the rules did not load (2 wins over 1) |

`check` prints a could-not-run reason to stderr as `<id>: could not run: <reason>`. It reads the repository the current directory is in.

`--ci` never fetches. It requires each source clone to sit at the commit in `rules.lock` (`git rev-parse HEAD`). A missing lockfile, a missing entry, or a mismatch exits 2.

Without `--ci`, a source that is not cloned exits 2 and asks for `rules sync`.

## Replay

`oncebitten::replay` proves an ast-grep or diff rule against its files. It takes a rule directory with `check.yml` (or `rule.toml` with `engine = "diff"`), `fixtures/bad.*`, `fixtures/fixed.*` and a non-empty `corpus/`. A diff fixture is a list of changed paths, one per line; the checker runs in `diff` mode and reads it.

- The rule is proven when the checker reports `fired` for `bad.*` and `clean` for `fixed.*` and every `corpus/` file.
- The checker is the `oncebitten-checker` binary. It prints one JSON line and exits 0. A result counts only when the JSON, the target path and exit 0 agree. Any other result is could-not-run. An exit code alone never means fired.
- Missing rule file, fixture or corpus, a missing checker, a sandbox or gateway failure, and a checker error are could-not-run. A rule that misses `bad.*`, or fires on `fixed.*` or a corpus file, is rejected.
- `OpenShell` runs the replay in an OpenShell sandbox. It builds an image per replay from `nvcr.io/nvidia/base/ubuntu:24.04` with the checker, rule and files baked in, creates the sandbox with a read-only policy, runs the checker once per file, and deletes the sandbox. The gateway limits sandbox names to 19 characters.
- Build the checker for the image as a static musl binary: `cargo build --release --target x86_64-unknown-linux-musl --bin oncebitten-checker` (needs a musl C toolchain; the replay tests used a `rust:1-alpine` container).
- Diff rules run in the sandbox like ast-grep rules. The diff engine matches paths only and reads no repository file, so a local run gives the same verdict; the sandbox run is the one that writes a proof.
- Steer rules replay with the matcher in the running `oncebitten` binary, not in a sandbox: `fixtures/bad.json` must trigger, `fixtures/fixed.json` and every `corpus/*.json` must not, and `corpus/` may be empty. The proof names `"backend": "matcher"` and the binary's hash. See [steer.md](steer.md).
- Native-linter rules are exempt from replay proofs until replay can run their toolchain in the sandbox; the gate still runs them.

### `oncebitten replay`

```sh
oncebitten replay <rule-id> [--checker <path>] [--gateway <endpoint>] [--local]
```

`<rule-id>` is the qualified id that `rules list` prints. The command loads rules as `check` does, replays that rule in the OpenShell sandbox and prints the verdict.

| Verdict | Output | Exit |
|---|---|---|
| proven | `<id>: proven; wrote <path>` on stdout | 0 |
| rejected | `<id>: rejected: <reason>` on stdout; any old `proof.json` is deleted | 1 |
| could not run | `<id>: could not run: <reason>` on stderr; `proof.json` is left as it was | 2 |

- `--checker`: the `oncebitten-checker` binary baked into the image. Default: `oncebitten-checker` next to the running `oncebitten`. The image runs Linux, so use the static musl build.
- `--gateway`: the OpenShell gateway endpoint. Default: the `openshell` CLI's own.
- `--local`: run the checker on the host, with no sandbox. It prints the verdict (`proven by the local runner; no proof written`) and writes no proof.

### Proofs

A proven sandbox replay writes `proof.json` into the rule directory, beside `rule.toml`. Commit it with the rule. A rule from a rules repo carries its proof in that repo, so replay it there and commit the proof to the rules repo.

```json
{
  "rule": "py-utcnow",
  "dir_sha256": "<SHA-256 of the rule directory>",
  "checker_version": "0.0.0",
  "checker_sha256": "<SHA-256 of the checker binary>",
  "backend": "openshell",
  "timestamp": 1794700000,
  "result": "proven"
}
```

- `rule` is the `id` in `rule.toml`, without a source prefix, so a proof stays valid when the directory moves between the project and a rules repo.
- `dir_sha256` hashes every file in the rule directory except the top-level `proof.json`: files sorted by their `/`-separated relative path, each contributing its path, a NUL byte, its length as a little-endian 64-bit integer and its bytes, with CRLF read as LF. A Windows checkout with CRLF line endings therefore hashes like a Unix one. A symlink in the directory makes the hash fail.
- `checker_version` is the version of the `oncebitten` that ran the replay; the checker builds from the same crate. `checker_sha256` names the exact checker binary. Validity does not compare either with the current checker, because CI need not have the musl checker.
- `timestamp` counts seconds since the Unix epoch.

A proof is valid when it parses, `result` is `proven`, `backend` is `openshell`, `rule` equals the rule's id and `dir_sha256` equals the directory's hash now. Any edit to the rule, its check file, fixtures or corpus makes the proof stale; replay again.

Only an OpenShell replay writes a proof. `--local` never does, and a hand-written proof with another backend counts as not proven. On Windows the replay needs WSL 2 with OpenShell; run `oncebitten replay` inside WSL. Windows without WSL cannot produce a proof.

Shipped examples are extracted to the data directory on each machine, so a proof written there stays on that machine. To prove an example for CI, copy it into `.oncebitten/rules/` under an id of its own, replay it and commit the proof.
