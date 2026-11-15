# Gate

`oncebitten gate` blocks a push or a pull request that adds a violation of an active block rule. It has two stages.

| Command | Runs from | Rules load | Missing or stale proof |
|---|---|---|---|
| `oncebitten gate pre-push [--base <ref>]` | the pre-push hook | as `check` does; a source that is not cloned exits 2 | a warning on stderr; the gate continues |
| `oncebitten gate pr --base <ref>` | CI, on a pull request | as `--ci`: `rules.lock` required, never fetches | exit 2 |

The pre-push stage runs through the [pre-commit](https://pre-commit.com) framework; oncebitten installs no git hook of its own. [prek](https://github.com/j178/prek) 0.1.5 or later (`uv tool install prek`, then `prek install --hook-type pre-push`), `uvx prek install --hook-type pre-push` and `uvx pre-commit install --hook-type pre-push` (uv: `curl -LsSf https://astral.sh/uv/install.sh | sh`) install the same hook; each blocked a violating push with `oncebitten-gate-system` and prek 0.5.0.

## Pre-push hook

Add the hook to `.pre-commit-config.yaml` and install the pre-push hook type:

```yaml
repos:
  - repo: https://github.com/bcmyguest/oncebitten
    rev: <commit or tag>
    hooks:
      - id: oncebitten-gate
```

```sh
pre-commit install --hook-type pre-push
```

The hook, in this repository's [`.pre-commit-hooks.yaml`](../.pre-commit-hooks.yaml), runs `oncebitten gate pre-push` once per push (`pass_filenames: false`, `always_run: true`, `stages: [pre-push]`). It needs pre-commit 3.2.0 or later, the first release with the `pre-push` stage name.

It uses `language: rust`. On first use pre-commit clones this repository and runs `cargo install --bins --root <env> --path .` in it, which builds `oncebitten` and `oncebitten-checker` into a cached environment (source: `pre_commit/languages/rust.py`, read in pre-commit 4.6.0). It uses the `cargo` on `PATH`, or installs rustup when there is none. The build compiles tree-sitter grammars, so it needs a C compiler. `language: rust` works for anyone with a Rust toolchain or network access.

`oncebitten-gate-system` is the same hook with `language: system`: it runs the `oncebitten` on `PATH` and builds nothing. Install the binary first with `install.sh` or `install.ps1` (see [install.md](install.md)), at a version that matches `rev`.

Checked by running `pre-commit try-repo <this repo> oncebitten-gate --hook-stage pre-push --from-ref <a> --to-ref <b>` with pre-commit 4.6.0 on Linux.

## CI

`templates/` holds a gate job for three CI systems. Each checks out full history, installs the release binary with `install.sh` from the release's tag, pinned with `ONCEBITTEN_VERSION`, and runs `oncebitten gate pr --base origin/<target branch>`.

| File | Target branch from | Make it required |
|---|---|---|
| [`templates/github-actions.yml`](../templates/github-actions.yml) | `github.base_ref` | branch protection: require the check `oncebitten gate` |
| [`templates/gitlab-ci.yml`](../templates/gitlab-ci.yml) | `CI_MERGE_REQUEST_TARGET_BRANCH_NAME` | merge request setting "Pipelines must succeed" |
| [`templates/azure-pipelines.yml`](../templates/azure-pipelines.yml) | `System.PullRequest.TargetBranch`, `refs/heads/` stripped | Azure Repos: build validation policy; GitHub: require the check |

Pin the install with `--rev <commit>` so the gate does not change under you. The templates have not run on GitLab or Azure; `tests/gate.rs` checks only that each one calls `oncebitten gate pr --base`.

`gate pr` needs `.oncebitten/rules.lock` committed, even with no sources: run `oncebitten rules sync` once and commit the file. A project with rule sources needs each clone at its locked commit before the gate runs: run `oncebitten rules sync --frozen` before `gate pr`. It checks out the locked commits, fetching one only when the clone lacks it. It never writes the lock and exits 2 when it cannot. Plain `rules sync` moves the lock to `rev` or `origin/HEAD`, so CI does not run it. The shipped templates and this repo's `ci.yml` run the frozen sync before the gate.

The gate reads rules, proofs and the baseline from the pull request's own checkout, so a pull request can change what gates it. Review changes under `.oncebitten/` with that in mind.

This repository runs the gate on its own pull requests: the `gate` job in `.github/workflows/ci.yml`, check name `oncebitten gate`.

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

The gate drops paths under `.oncebitten/rules/`: a rule's `fixtures/bad.*` exists to violate it. `rules.lock` and `baseline.json` stay in the range. Untracked files never count. The engines read the files in the working tree, as pre-commit hooks do: a pre-push run with uncommitted edits to a changed path checks the edited file. CI checks out the pull request, so there the working tree equals `HEAD`.

The `native` engine runs over the whole repository whatever the range. A `diff` rule sees the range's paths.

## Proofs

The gate needs a valid `proof.json` (see [rules.md](rules.md#proofs)) for each gated `ast-grep` and `diff` rule. Ast-grep and diff rules need a proof. So does every active steer rule: the gate never runs it, but it acts in every agent session, so a stale or unproven one fails `pr` the same way (see [steer.md](steer.md#fixtures-and-proofs)). Native-linter rules are exempt until replay can run their toolchain in the sandbox; the gate still runs them.

The proof's `dir_sha256` covers every file in the rule directory except `proof.json`, with `rule.toml` parsed, its `status` key removed and the rest re-serialised canonically. So `status = "proposed"` becomes `"active"` without invalidating the proof. Any other change to `rule.toml`, comments and layout aside, makes the proof stale.

For a steer rule the gate also replays the fixtures with its own matcher and fails when the replay does not prove, whatever `proof.json` says. For other rules, where the proof records a `checker_sha256`, the gate compares it with the `oncebitten-checker` beside the running binary; a different hash makes the proof stale. When that file is absent, as in CI without the musl checker, the comparison is skipped. A steer proof is not tied to a binary: the local replay is the check.

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

### `oncebitten baseline update`

```sh
oncebitten baseline update
```

Replaces `.oncebitten/baseline.json` with the violations that the active block rules find now. It loads rules as `check` does and runs them on every tracked file, minus `.oncebitten/rules/`. Diff rules do not run. It prints `wrote <n> entries to <path>` and exits 0. A rule that could not run exits 2 and writes nothing; it never writes a partial baseline. A violation fixed since the last update drops out.

The file is pretty-printed JSON, entries sorted by `rule`, then `path`, then `sha256`, one key per line, ending in a newline. Running it twice on the same tree writes the same bytes, so a baseline change reads as a short diff.

A pull request can edit the baseline. Review a change to `baseline.json` like a change to the rules.

A retired rule never runs, so `baseline update` drops its entries. A finding a live exception covers stays out of the baseline.

## PR attachments

`gate pr` only. When a `diff` rule's `when_changed` matched but `require_changed` did not, the gate reads the description and comments of the open pull request for the current branch through the forge (`gh`, `glab` or `az repos`; see [configuration.md](configuration.md)). The rule passes when the text holds an attachment link whose extension matches a `require_changed` glob (`docs/screenshots/*.png` accepts a `.png` link; `*.{png,jpg}` accepts either). Link forms: a markdown image or link, a bare URL ending in the extension, and an HTML `<img src=...>`. A GitHub `user-attachments/assets/<uuid>` URL has no extension; inside a markdown image or `<img>` it counts as an image and matches the globs `png`, `jpg`, `jpeg`, `gif`, `webp` and `svg`. A glob with no literal extension, such as `docs/**`, accepts no link.

With no forge, no open pull request or a forge failure, the rule fires on the diff alone and the gate prints one `warning:` line to stderr. This never exits 2. `gate pre-push` never calls the forge. Azure DevOps: only the description is read (see `src/forge.rs`).

## Exceptions

`.oncebitten/exceptions.toml` lets a rule's findings through on named paths until a date; [lifecycle.md](lifecycle.md#exceptions) has the format. Both stages drop findings a live exception covers before the baseline applies and print `<n> violation(s) excepted by .oncebitten/exceptions.toml` to stderr. An expired exception covers nothing; the gate prints `warning: expired exception: <rule> on <paths> until <date>: <reason>` for each one. A file that does not parse exits 2.

## Proposals notice

Both stages print `<n> rule proposal(s) waiting: oncebitten proposals list` to stderr when triage queued proposals for this repo. The notice never changes the exit code. See [proposals.md](proposals.md#gate-notice).

## Exit codes

| Exit | Meaning |
|---|---|
| 0 | no new violation; for `pr`, every gated ast-grep rule has a valid proof |
| 1 | a gated rule fired on a violation that neither the baseline nor a live exception covers |
| 2 | could not run: rules did not load, a git command failed, the baseline or `exceptions.toml` did not parse, a rule could not run, or (`pr` only) a proof is missing or stale. 2 wins over 1. |
