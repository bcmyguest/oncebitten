# Evidence

`oncebitten evidence` groups the signals that `capture` recorded (see [capture.md](capture.md)) into mistake classes, counts independent occurrences and remembers rejections. It makes no model call. Triage (PLAN.md step 8) reads the same clusters.

Evidence is temporary. `proposals accept` and `proposals reject` delete a cluster's events and keep one record of its counts, and `capture` deletes any other event older than `evidence.ttl_days` (default 30) unless a waiting proposal names it.

## Clusters

Each signal maps to a class and a key. Signals with the same key form one cluster.

| Class | Signals | Key | Source |
| --- | --- | --- | --- |
| `failure` | `tool-failure`, `repeat-failure` | tool + normalized command | mined |
| `test-fail-then-pass` | `test-fail-then-pass` | tool + normalized command | mined |
| `revert` | `revert` | tool + normalized command | mined |
| `user-correction` | `user-correction` | prompt text | stated |

- Normalization is the one `repeat-failure` uses: whitespace collapsed, digits, paths and hex replaced with `<n>`, `<path>` and `<hex>`.
- A cluster id is the first 12 hex digits of the SHA-256 of its key, so the same mistake has the same id on every machine.
- A `user-correction` cluster shows the matched phrases as its pattern, never prompt text. Only identical prompts share a cluster.

## Dedupe and counts

- Signals on one event are one occurrence. A `tool-failure` and the `repeat-failure` on the same event count once.
- Occurrences in one session are correlated. `sessions` counts distinct (host, session id) pairs; that is the independent occurrence count.
- A mined cluster is a candidate from 2 sessions, a stated cluster from 1: a user who says "never" needs to say it only once.

## Provenance

`evidence show <id>` prints, for each occurrence: timestamp (ms), host, session id, repo (or `-`), event id, its signals, the stored summary and the event's `git_head` (or `-`). The header lists the source and the distinct repos.

## Rejection

`evidence reject <id> --reason <text>` records the cluster id, time and reason in the `rejections` table. A rejected cluster is never a candidate again, even as new occurrences arrive. The reason must be non-empty and the id must name an existing cluster. Rejecting again replaces the reason. `proposals reject` records the same rejection (see [proposals.md](proposals.md)).

## Commands

```sh
oncebitten evidence list          # candidates: id, class, status, sessions, occurrences, tool, pattern
oncebitten evidence list --all    # also below-threshold and rejected clusters
oncebitten evidence show <id>     # one cluster with each occurrence's provenance
oncebitten evidence reject <id> --reason "one-off"
```

Status is `candidate`, `below-threshold`, `accepted` or `rejected`. A decided cluster stays listed after its events are gone; `evidence show` then prints the decision and its counts. Clusters print most sessions first, then by id.

## Storage

Clusters are computed on read from `events` and `signals`; rejections and decisions are stored. The `decisions` table, created with `IF NOT EXISTS`, holds per cluster the outcome, rule, reject reason, occurrence and session counts, first and last seen timestamps and the decision time. Schema 3 (`PRAGMA user_version = 3`) adds the `rejections` table. Schema 4 (`PRAGMA user_version = 4`) adds `events.git_head`; opening a schema 0 to 3 database migrates it.

## Provenance and dedupe

Each stored event records its host, session id, repo (the directory holding `.git`, found from the hook's `cwd`), timestamp, kind and tool, plus `git_head`: the output of `git -C <repo> rev-parse --verify -q HEAD` at capture time, or null outside a repo or before the first commit. `git_head` is not part of any key below.

The dedupe key is stored in `events.payload_hash`, which has a UNIQUE constraint. A duplicate insert stores no event and no signals.

- If the hook payload has a `tool_use_id`, the key is SHA-256 over (host, session id, kind, tool_use_id). A redelivered hook repeats the id, so it is stored once even when its capture time differs.
- Otherwise the key is SHA-256 over the whole event: timestamp, host, session id, repo, kind, tool and summary. Two identical prompts at different times differ by timestamp and are both kept. A prompt redelivered at a different millisecond is stored twice; prompts carry no id to dedupe on.

## Source

`src/evidence.rs` holds the key and grouping rules. `Store::signal_rows` in `src/store.rs` reads the signals joined to their event tool. The CLI is in `src/main.rs`. Tests: `tests/evidence.rs`, `tests/evidence_cli.rs`.
