# Roadmap

oncebitten ships in twelve steps, grouped into four milestones. Each step ends with its tests passing and user sign-off. A review closes each milestone. This page follows the step list in [PLAN.md](../PLAN.md).

Status as of 2026-11-16:

| Milestone | Steps | Status |
| --- | --- | --- |
| M1 | 0-1 | Done |
| M2 | 2-5 | Done |
| M3 | 6-9 | Done |
| M4 | 10-11 | Done |

"Done" means merged to `main`. "In progress" means work on a branch that is not merged yet.

## M1: steps 0-1

| Step | What it delivers | Status |
| --- | --- | --- |
| 0. Scaffold | The Rust crate, layered TOML configuration and this docs skeleton. | Done |
| 1. Spikes | Written experiments, code thrown away: replay one ast-grep rule in OpenShell from Rust; ast-grep with Python, Rust and JavaScript grammars; one headless JSON call per agent host. See [research/](research/). | Done |

## M2: steps 2-5

| Step | What it delivers | Status |
| --- | --- | --- |
| 2. Registry | One TOML file per rule, `oncebitten rules list` and `rules validate`. Rules from several repositories, with repo-prefixed ids, explicit overrides and pinned revisions; the same result locally and in CI. See [Rules](rules.md). | Done |
| 3. Block engine | Checks that run on ast-grep, on native linters, or on the diff (`when_changed` requires `require_changed`). Inactive example rules with bad and fixed fixtures, and `llms.txt`. Rust `unwrap` ships as the clippy `unwrap_used` lint. | Done |
| 4. Replay | `oncebitten replay` proves a rule in a sandbox: it fires on the bad fixture and passes on the fixed one and on a valid corpus. A run that cannot complete reports "could not run", never "fired". | Done |
| 5. Gate | `oncebitten gate pre-push` and `gate pr`, a pre-commit hook and a required CI check. See [Gate](gate.md). | Done |

## M3: steps 6-9

| Step | What it delivers | Status |
| --- | --- | --- |
| 6. Signal capture | Hooks for Claude Code, Codex, Cursor, opencode and pi that record signals locally, with no model calls. See [Signal capture](capture.md). | Done |
| 7. Evidence | A store that clusters signals, with provenance, deduplication, independent occurrence counts and memory of rejected clusters. See [Evidence](evidence.md). | Done |
| 8. Triage | A headless call to an agent host reviews the evidence and returns JSON, with its hooks off, a timeout and bounded context. oncebitten refuses a host that fails the hooks-off proof; opencode is exempt from that proof. | Done (PR #9) |
| 9. Proposals | A proposed rule arrives as a commit in the current pull request or as a pull request to the rules repository. A proof gone stale replays again. 9a: triage drafts the rule, proposals queue unproven or proven with provenance, `oncebitten proposals`, gate notice, background triage ([Proposals](proposals.md)). 9b: forge delivery, `replay --stale` and PR attachments as diff-rule evidence. | Done (PRs #14 and this one) |

## M4: steps 10-11

| Step | What it delivers | Status |
| --- | --- | --- |
| 10. Steer mode | Steer rules and their proofs, and routing for lessons git cannot see. | Done (PR #13) |
| 11. Lifecycle | Rule exceptions with a reason and an expiry date, retirement, rollback and `oncebitten doctor`. | Done (PR #12) |

## Experiments

| Experiment | State |
| --- | --- |
| Prompt-intent classifier | Built behind the cargo feature `intent`, off by default and wired to nothing. It finds 0 of 25 rule-request and 0 of 25 correction test rows at a false-positive rate of 5% or less ([research](research/intent-classifier.md)). Regex signals stay the capture path. |
