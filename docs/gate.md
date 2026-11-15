# Gate

`oncebitten gate` blocks a push or a pull request that adds a violation of an active block rule. It has two stages.

| Command | Runs from | Rules load | Missing or stale proof |
|---|---|---|---|
| `oncebitten gate pre-push [--base <ref>]` | the pre-push hook | as `check` does; a source that is not cloned exits 2 | a warning on stderr; the gate continues |
| `oncebitten gate pr --base <ref>` | CI, on a pull request | as `--ci`: `rules.lock` required, never fetches | exit 2 |

## What it checks

The gate runs every rule with `status = "active"` and `mode = "block"`. Steer rules, proposed rules and retired rules do not gate. It reuses the engines of `oncebitten check`, so a finding prints the same way: `<id>: <path>[:<line>]: <message>` on stdout.

It checks the paths that changed in a range of commits:

| Stage | Range |
|---|---|
| `pr` | `git diff --name-only <merge-base of --base and HEAD> HEAD` |
| `pre-push --base <ref>` | the same, with `<ref>` |
| `pre-push` | `PRE_COMMIT_FROM_REF` to `PRE_COMMIT_TO_REF` |
| `pre-push`, refs unset | merge-base of `@{upstream}` and `HEAD`, to `HEAD` |
| `pre-push`, no upstream | every tracked file (`git ls-files`) |

The pre-commit framework sets `PRE_COMMIT_FROM_REF` (the remote's commit) and `PRE_COMMIT_TO_REF` (the local commit) in the pre-push stage. Source: `pre_commit/commands/run.py` and `_pre_push_ns` in `pre_commit/commands/hook_impl.py` (read in pre-commit 4.6.0). It leaves them unset when a push contains the repository's root commit; it then runs hooks on all files, and the gate falls back to `@{upstream}` or every tracked file.

Untracked files never count. The engines read the files in the working tree, as pre-commit hooks do: a pre-push run with uncommitted edits to a changed path checks the edited file. CI checks out the pull request, so there the working tree equals `HEAD`.

The `native` engine runs over the whole repository whatever the range. A `diff` rule sees the range's paths.

## Proofs

The gate needs a valid `proof.json` (see [rules.md](rules.md#proofs)) for each gated `ast-grep` rule. Replay supports ast-grep rules only, so `native` and `diff` rules need no proof until replay supports them.

A missing, stale or not-proven proof prints `<level>: <id>: <reason>` to stderr. `pr` prints `error` and exits 2 after running the rules. `pre-push` prints `warning` and exits as if the proof were valid.

A shipped example from `rules.use_examples` is active, but its directory sits in the data directory, so CI has no proof for it and `gate pr` exits 2. Copy the example into `.oncebitten/rules/`, replay it and commit the proof.

## Baseline

`.oncebitten/baseline.json` lists violations that predate the gate. A finding that matches an entry is known: it does not fail the gate, and the gate prints `<n> known violation(s) matched .oncebitten/baseline.json` to stderr. A missing file is an empty baseline; a file that does not parse exits 2.

```json
{
  "entries": [
    {
      "rule": "py-utcnow",
      "path": "src/app.py",
      "sha256": "<SHA-256 of the matched text>"
    },
    {
      "rule": "lint-clean"
    }
  ]
}
```

- `rule` is the qualified id that `rules list` prints.
- An `ast-grep` entry has `path`, repo-relative and `/`-separated, and `sha256`, the SHA-256 of the matched source text with CRLF read as LF. The line number is not part of the entry, so a known violation stays known when code above it moves. Two identical matches in one file share one entry: a baselined match lets a second copy of the same text in the same file through. A copy in another file is new.
- A `native` entry has `rule` only. The native engine reports one finding for the whole repository, with no path and no per-match text, so the entry covers every firing of that rule: while it is listed, the rule cannot fail the gate.
- A `diff` finding is never baselined. It belongs to one change, so an entry cannot match it.

A pull request can edit the baseline. Review a change to `baseline.json` like a change to the rules.

## Exit codes

| Exit | Meaning |
|---|---|
| 0 | no new violation; for `pr`, every gated ast-grep rule has a valid proof |
| 1 | a gated rule fired on a violation the baseline does not list |
| 2 | could not run: rules did not load, a git command failed, the baseline did not parse, a rule could not run, or (`pr` only) a proof is missing or stale. 2 wins over 1. |
