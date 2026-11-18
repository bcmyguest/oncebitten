# Proposals

Triage turns an evidence cluster into a queued result (see [triage.md](triage.md)). A verdict that names a rule-shaped mistake carries a drafted rule; oncebitten writes the draft as a proposed rule, replays it, and queues it for review. Proposals never become active by themselves.

## From cluster to proposal

<p align="center">
<picture>
<source media="(prefers-color-scheme: dark)" srcset="media/proposals-flow-dark.svg" />
<img src="media/proposals-flow.svg" alt="Flowchart: a cluster reaching its threshold, or oncebitten triage id, yields a verdict. A rule-shaped verdict with a valid draft is written to .oncebitten/rules.local/id/ as proposed and replayed in OpenShell: proven writes proof.json, while no sandbox, rejection, or a failed run leaves it unproven. A verdict without a draft queues a finding." width="624" />
</picture>
</p>

- A verdict is rule-shaped when `repeatable` is true, `proposed_fix_kind` is not `none` and `generality` is not `one-off`. A draft beside any other verdict is dropped; the verdict queues as a finding.
- An invalid draft rejects the whole answer as "could not run": nothing is printed or written.
- The draft lands in the repo's local layer, `.oncebitten/rules.local/<id>/`, the repo being the git root of the triage working directory. The layer is gitignored (see [rules.md](rules.md#local-rules)): an accepted draft is an active rule on this machine only, until `proposals deliver` shares it. A draft whose id matches a loaded rule, or that breaks the rule set, is not written.
- Replay runs as `oncebitten replay` does: a proven verdict writes `proof.json`. Without a sandbox, or when replay rejects the rule or cannot run, the proposal stays unproven and its reason is kept. When `openshell` is not on `PATH`, `oncebitten triage <id>` adds one stderr line saying so; see [doctor.md](doctor.md).
- A cluster queues at most once, with one exception: a manual `oncebitten triage <id>` on a cluster whose row is a waiting finding asks the host again and, when the answer carries a valid draft, replaces the finding with the proposal in one transaction. Without a draft the finding stays. Background triage never replaces a row, and a queued proposal or a decided cluster is never replaced. A rejected cluster is never queued.

## Draft

The draft is the optional `draft` key of the triage answer, `null` when no rule fits.

| Field | Meaning |
|---|---|
| `id` | lowercase kebab-case, at most 64 characters; the rule directory name |
| `description`, `rationale` | 1 to 1000 characters |
| `mode` | `block` or `steer` |
| `engine` | `ast-grep`, `diff`, `native` or `steer` |
| `language` | optional |
| `check` | ast-grep only: the `check.yml` text |
| `command`, `fired_exit_codes`, `fired_output_contains` | native only |
| `when_changed`, `require_changed` | diff only, non-empty glob arrays |
| `prompt` | prompt steer only: a regex on the user's prompt, at most 1000 characters |
| `bad`, `fixed` | `{name, content}`; named `bad.<ext>` and `fixed.<ext>` (diff: `.diff`), at most 64 KiB each |
| `corpus` | up to 10 `{name, content}` files the check must pass; replay needs at least one |

Validation is as strict as for a hand-written rule: the written `rule.toml` must load, `check.yml` must compile, globs must parse, and file names must be plain names that Windows can also write (no `<>:"|?*` or control characters, no trailing dot or space, no reserved device names such as `nul`, `con` or `com1`). A write that fails midway removes the rule directory. Keys for another engine are rejected.

### Steer drafts

A draft with `engine = "steer"` must have `mode = "steer"`. The reverse is not refused: a steer-mode draft with another engine still parses. A steer draft is a command steer, or a prompt steer when `prompt` is set; see [Prompt steer drafts](#prompt-steer-drafts). A command steer's fixtures come from the cluster's evidence, not the model:

- `bad` is the cluster's latest recorded command, as a `{"event": "tool", "tool", "command"}` event.
- A user-correction cluster's evidence is a prompt, not a command. Its `bad` is the latest command in the corrected turns (see [triage.md](triage.md#call-contract)) that the model's own `bad` command matches once turned into the trigger below. With no such command nothing is written and triage warns `not queued`.
- The trigger is `bad`'s tool plus a regex on its program, its first subcommand (a word before any flag) and its first flag, so `git push --force origin main` also fires on `git -C repo push origin dev --force` but not on `git push origin main` or `git push --force-with-lease`. Leading `NAME=value` assignments and a path on the program are ignored.
- The corpus is up to 5 other successful commands from the same repo in the store. Repo paths compare in canonical form, so a Windows `\\?\` prefix or a macOS `/private` symlink still matches. With none, the model's corpus is kept.
- A steer draft is proven when it is queued, with the matcher in the running binary. It needs no OpenShell sandbox; every other engine does.
- A steer draft is exempt from the rule that `bad` must quote the changed code sent to triage: its fixtures are recorded events.
- Every steer draft writes `action = "inform"`. The draft has no `action` key, so a model request for `deny` fails to parse. Use `accept --deny` to upgrade a command steer.

### Prompt steer drafts

A steer draft with `prompt` set triggers on the user's prompt and speaks when the agent tries to finish the turn. It covers agent behaviour that no check on changed files and no single command catches. The model supplies everything:

```json
{
  "id": "address-review-comments",
  "description": "check every review comment before finishing",
  "mode": "steer",
  "engine": "steer",
  "prompt": "(?i)review comments?|feedback",
  "bad": {"name": "bad.json", "content": "{\"event\": \"prompt\", \"prompt\": \"address the review comments on my PR\"}"},
  "fixed": {"name": "fixed.json", "content": "{\"event\": \"prompt\", \"prompt\": \"review this function for bugs\"}"},
  "corpus": [{"name": "tests.json", "content": "{\"event\": \"prompt\", \"prompt\": \"run the tests\"}"}],
  "rationale": "Before you finish, check that every review comment is addressed."
}
```

- The written rule has `mode = "steer"`, `engine = "steer"`, `[steer] action = "inform"`, `at = "stop"`, `message` set to the draft's `rationale`, and `[steer.trigger] prompt` set to the regex. See [steer.md](steer.md#at-stop).
- `bad` is one user prompt that must trigger, `fixed` one prompt on a nearby topic that must not, and `corpus` up to 10 ordinary prompts that must not. Each is a `{"event": "prompt", "prompt"}` event in a `.json` file. oncebitten keeps them as the model wrote them.
- The regex must compile with the Rust `regex` crate, must not match the empty string, must match `bad` and must match neither `fixed` nor any corpus prompt. The matcher replay checks this when the answer is parsed; a failure is "could not run" and nothing is written.
- A draft with `prompt` set whose fixture records a tool event is rejected: a draft is a command steer or a prompt steer, never both. `prompt` on another engine is rejected too.
- It is proven with the matcher when queued and exempt from quoting changed code, as a command steer is.
- `accept --deny` refuses it: deny needs a tool trigger.

## Commands

```
oncebitten proposals list
oncebitten proposals show <cluster-id>
oncebitten proposals accept <cluster-id> [--deny]
oncebitten proposals reject <cluster-id> --reason <text>
oncebitten proposals reject <cluster-id> --duplicate-of <other-cluster-id>
```

Proposals are keyed by their evidence cluster id.

- `list` prints `cluster  state  kind  rule  proof  hits  repo` per queued result. State is `waiting`, `accepted` or `rejected`; kind is `proposal` or `finding`; proof is `proven`, `unproven` or `-`; hits is `N hits in M files` or `-`.
- `show` adds the rule directory, the hits with up to three sample lines (`path:line: text`), rationale, proof note, verdict JSON and the occurrence event ids that produced it. After a decision it prints the kept counts instead, because the events are gone.
- `accept` flips `status = "proposed"` to `status = "active"` only when the rule has a valid proof. Otherwise it exits 2 with the reason: no proof, a stale proof, or a finding. A project draft is committed with its `proof.json`; a local draft is gitignored, so share it with `oncebitten proposals deliver`. On activation, `accept` also writes, in the proposal's own repo whatever the cwd, one baseline entry per existing hit of that rule (in `.oncebitten/baseline.local.json` as `local/<id>` for a local rule, else in `.oncebitten/baseline.json`), and keeps the entries of other rules. Later commits that touch those lines pass the gate; a new hit fails it.
- `accept --deny` sets a steer proposal's `[steer]` action to `deny`, re-proves it with the running binary, then activates it. It exits 2 for a non-steer proposal and for a prompt steer proposal, which can only inform.
- `reject` records the rejection in the evidence store (as `evidence reject` does) and removes the rule directory if it is still proposed. The cluster is never proposed again.
- `reject --duplicate-of <other>` rejects the cluster as a duplicate of `<other>`, which must be a queued or rejected proposal other than the cluster itself. It rejects the cluster with the reason `duplicate of <other>`, then stores the link in the `duplicate_of` column; a failed reject stores no link. It never merges or drops another proposal.
- `accept` and `reject` delete the cluster's captured events and keep one provenance record: outcome, rule, reason, occurrence and session counts, first and last seen, decision time. The `rejections` table, not the events, keeps a rejected cluster from returning.

Exit 0 on success, 2 on any error.

## Possible duplicates

When a proposal is recorded, it is compared with every row in the `proposals` table except itself, whatever its state. Two checks run, and the first match wins:

1. Equal `mistake_class` (reason `same mistake_class`). Triage shows the host the classes already named, so the same mistake reuses one slug; see [triage.md](triage.md).
2. Text similarity of the rationale at or above 0.5 by normalised token Jaccard (reason `similar text`). Tokens are lowercase alphanumeric runs. The score is `|A ∩ B| / |A ∪ B|`. "never use unwrap in library code" and "stop using unwrap in library code" score 0.5 and match. "never use unwrap in code" and "never run clippy in code" score 0.43 and do not.

A match is shown as `possible duplicate of <id> (<reason>)` in `proposals list`, `proposals show`, and the forge PR body. The match is advisory. Nothing merges or drops automatically. Confirm a duplicate with `proposals reject <id> --duplicate-of <other>`, which shows as `confirmed duplicate of <other>`.

## Gate notice

`gate pre-push` and `gate pr` print one line to stderr when proposals wait for the current repo:

```
2 rule proposals waiting: oncebitten proposals list
```

The notice never changes the gate's exit code, and the gate never creates the evidence store.

## Session-start notice

On claude and codex the steer hook answers `SessionStart` with the count and ids of the rule proposals waiting for the session's repository, and tells the agent to show each to the user, ask whether to accept or reject it, and run `proposals accept` or `proposals reject`. Findings do not count. The hook prints nothing when no proposal waits, and never creates the evidence store. `oncebitten hooks install` and the claude and codex plugins install it. See [steer.md](steer.md#session-start-notice).

## Background triage

When `capture` records an event that brings a cluster to its threshold (see [evidence.md](evidence.md)), it starts `oncebitten triage <id> --queue` detached in the event's repo and returns at once. The capturing host is asked when it has a configured target; otherwise the `triage.default` target. Background triage:

- takes a per-cluster lock in the store, so one triage per cluster runs; a lock older than 30 minutes is taken over;
- skips a cluster that is already queued or rejected without asking the host;
- prints nothing and uses the triage timeout (120 seconds per host call);
- stores the error of a failed run, which `oncebitten proposals list` prints to stderr as `triage failed for <id> (<host>): <error>; retry: oncebitten triage <id>` until a later run of that cluster or a queued proposal replaces it.

Capture ignores events outside a git repository for this purpose and never fails the host's hook because of triage. Set `triage.background = false` to turn it off (see [configuration.md](configuration.md)).

## Store

Two tables in the evidence database, created on open: `proposals` (one row per cluster: repo, kind, state, rule id and directory, proof flag and note, verdict JSON, rationale, occurrence ids, and after delivery the target, commit and PR URL) and `triage_locks`.

## Existing hits

When a proposal is recorded, its rule runs over the tracked files as they are on disk. The count is stored as `N hits in M files` with up to three sample lines. It informs reviewers and never blocks.

- Only `ast-grep` rules are counted. Native rules and diff rules show `-`.
- A scan that fails leaves the proposal queued with no hits.
- `list` shows the count; `show` shows the count and samples. The delivered PR body adds `Hits:` and the line `existing hits will be baselined`, which `accept` makes true in the proposal's own repo. A `rules-repo` PR qualifies the line: only the proposal's repo is baselined, and other repos using the rule get no baseline.

## Delivery

`oncebitten proposals deliver <cluster-id> [--to pr|rules-repo] [--push]` commits a proposed rule. It never sets the rule's status; `proposals accept` does.

<p align="center">
<picture>
<source media="(prefers-color-scheme: dark)" srcset="media/proposals-delivery-dark.svg" />
<img src="media/proposals-delivery.svg" alt="Flowchart of proposals deliver: a finding or rejected proposal exits 2. A rule with a missing or stale proof replays first, and an unproven rule still goes on. With no --to, an open PR for the branch or a forge error with no writable source decides between pr and rules-repo. pr commits the rule directory on the current branch, and rules-repo copies it into the writable source clone on branch oncebitten/propose-id. --push pushes and opens a PR; without it the rules-repo path prints the branch and push command." width="624" />
</picture>
</p>

- A stale or missing proof replays first, with the prover `triage` uses. A rule that stays unproven is still delivered; the commit message and PR body say `unproven: <reason>`.
- `pr` shares the rule with the team: a rule under `.oncebitten/rules.local/` moves to `.oncebitten/rules/<id>/` first, taking its `baseline.local.json` entries into `baseline.json` under the bare id and rewriting its `exceptions.toml` entries from `local/<id>` to `<id>`; both files are staged in the same commit when they changed. Everything moves back if the commit fails. `--to rules-repo` leaves both files alone. It commits only the rule directory and those files, each at its HEAD content plus the promoted rule's records, so other uncommitted edits to them stay out of the commit and in the working tree, with the message `chore(oncebitten): propose rule <id>` and a body that lists the cluster id, the rationale and the proof state. Nothing to commit exits 2. `--push` runs `git push` to the branch's upstream.
- `rules-repo` shares the rule across repositories and keeps the local copy, which yields to the source rule once that is merged and synced; the local copy's `local/<id>` baseline entries and exceptions then no longer apply, so rerun `baseline update` or delete the local copy. It needs the one source with `writable = true` (`rules.sources[].writable`) and that source cloned by `rules sync`. It copies the rule directory into the clone, creates `oncebitten/propose-<id>` there and commits. The clone stays on that branch until the next `rules sync`. With `--push` it pushes the branch and opens a PR against the source's `rev`, or origin's default branch; without it prints the branch and the push command.
- The forge is chosen as in [configuration.md](configuration.md) (`forge` key, else the `origin` remote). A forge CLI that is missing or exits non-zero is an error that carries its stderr.
- The store records the target, commit and PR URL; `proposals list` and `proposals show` print them. Older databases gain the three columns on open.
- The `Hits:` line and `existing hits will be baselined` line appear in the PR body when the proposal has hits. For `rules-repo` the line says that only the proposal's repo is baselined, by `oncebitten proposals accept <id>`.
- Exit 0 delivered; 2 a finding, a rejected proposal, no target, or any failure.
