# Rule lifecycle

A rule moves through three statuses, set by `status` in its `rule.toml`:

| Status | `check` and `gate` | Proof needed |
|---|---|---|
| `proposed` | never runs | no |
| `active` | runs; a `block` rule gates | yes, for ast-grep and diff rules in `gate pr` |
| `retired` | never runs | no |

`rules rollback` moves a rule from `active` to `proposed`; `rules retire` moves a project rule to `retired`. Exceptions let an active rule through on named paths until a date.

## Exceptions

`.oncebitten/exceptions.toml` lists findings a rule lets through:

```toml
[[exception]]
rule = "team/no-unwrap"          # qualified id, as `rules list` prints it
paths = ["src/legacy/**"]        # globs, repo-relative with `/`; required
reason = "Rewrite lands in Q1"   # required, non-empty
expires = "2027-03-31"           # required, YYYY-MM-DD, last day it applies (UTC)
```

- Every key is required. `paths` must name at least one glob; an exception never covers every path by default. Write `paths = ["**"]` to cover every path; only `**` covers a finding that names no path, such as a native rule's.
- Globs use the diff engine's syntax: `*` stays within one directory, `**` crosses directories.
- An exception is live through its `expires` date and expired from the next day. An expired exception covers nothing.
- `check`, `gate pre-push` and `gate pr` drop findings a live exception covers and print `<n> violation(s) excepted by .oncebitten/exceptions.toml` to stderr. `gate` prints `warning: expired exception: <rule> on <paths> until <date>: <reason>` for each expired one.
- `baseline update` leaves excepted findings out of the baseline, so they fail the gate again once the exception expires.
- A missing file means no exceptions. A file that does not parse, or an entry with a missing or empty key, an invalid glob or an invalid date, exits 2 in `check`, `gate` and `baseline update`.
- The gate reads the file from the pull request's checkout. Review a change to it like a change to the rules.

### Commands

| Command | Does | Exit |
|---|---|---|
| `oncebitten exception add <rule> --paths <glob>... --reason <text> --expires <YYYY-MM-DD>` | Append an `[[exception]]` entry; comments and layout above it stay | 0 added; 2 unknown rule, invalid entry or invalid file |
| `oncebitten exception list` | Print `rule  paths  expires  live\|expired  reason` per entry | 0; 2 invalid file |

`exception add` takes several globs as `--paths a/** b/**`. The rule must load: `exception add` refuses an id `rules list` does not print.

## Retire

```sh
oncebitten rules retire <id> --reason "Python 3.12 removed datetime.utcnow"
```

Sets `status = "retired"` and `retired_reason` in the rule's `rule.toml`, keeping its comments and layout. A retired rule never runs in `check` or `gate` and needs no proof. The next `baseline update` drops its entries from `baseline.json`.

- Only a project rule (`.oncebitten/rules/<id>/`) retires here. A source rule exits 2: retire it in its rules repo, or override it with a project rule that sets `overrides = "<source>/<id>"`. A shipped example exits 2: remove it from `rules.use_examples`.
- `--reason` must be non-empty. A rule already retired exits 2.
- `retired_reason` is optional in `rule.toml`; when present it must be non-empty.

## Rollback

```sh
oncebitten rules rollback <id>
```

Returns an active project rule to `proposed`, so it stops blocking at once. The rule's files and `proof.json` stay. Setting `status = "active"` again re-approves it, and the proof stays valid, because `status` is not part of the proof hash.

Rollback prints the git commands that restore an earlier version of the rule directory; it does not rewrite history itself:

```sh
git log --oneline -- .oncebitten/rules/<id>
git restore --source <commit> -- .oncebitten/rules/<id>
```

A restored rule needs a fresh `oncebitten replay` when its files differ from the proven ones. A rule that is not active exits 2; a source rule or shipped example exits 2 as in retire.
