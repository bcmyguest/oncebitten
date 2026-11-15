# Evidence

`oncebitten evidence` groups the signals that `capture` recorded (see [capture.md](capture.md)) into mistake classes, counts independent occurrences and remembers rejections. It makes no model call. Triage (PLAN.md step 8) reads the same clusters.

## Clusters

Each signal maps to a class and a key. Signals with the same key form one cluster.

| Class | Signals | Key | Source |
| --- | --- | --- | --- |
| `failure` | `tool-failure`, `repeat-failure` | tool + normalized command | mined |
| `test-fail-then-pass` | `test-fail-then-pass` | tool + normalized command | mined |
| `revert` | `revert` | tool + normalized command | mined |
| `user-correction` | `user-correction` | prompt digest | stated |

- Normalization is the one `repeat-failure` uses: whitespace collapsed, digits, paths and hex replaced with `<n>`, `<path>` and `<hex>`.
- A cluster id is the first 12 hex digits of the SHA-256 of its key, so the same mistake has the same id on every machine.
- A `user-correction` cluster shows the matched phrases as its pattern, never prompt text. Only identical prompts share a cluster.

## Dedupe and counts

- Signals on one event are one occurrence. A `tool-failure` and the `repeat-failure` on the same event count once.
- Occurrences in one session are correlated. `sessions` counts distinct (host, session id) pairs; that is the independent occurrence count.
- A mined cluster is a candidate from 2 sessions, a stated cluster from 1: a user who says "never" needs to say it only once.

## Provenance

`evidence show <id>` prints, for each occurrence: timestamp (ms), host, session id, repo (or `-`), event id, its signals and the stored summary. The header lists the source and the distinct repos.

## Rejection

`evidence reject <id> --reason <text>` records the cluster id, time and reason in the `rejections` table. A rejected cluster is never a candidate again, even as new occurrences arrive. The reason must be non-empty and the id must name an existing cluster. Rejecting again replaces the reason.

## Commands

```sh
oncebitten evidence list          # candidates: id, class, status, sessions, occurrences, tool, pattern
oncebitten evidence list --all    # also below-threshold and rejected clusters
oncebitten evidence show <id>     # one cluster with each occurrence's provenance
oncebitten evidence reject <id> --reason "one-off"
```

Status is `candidate`, `below-threshold` or `rejected`. Clusters print most sessions first, then by id.

## Storage

Clusters are computed on read from `events` and `signals`; only rejections are stored. Schema 3 (`PRAGMA user_version = 3`) adds the `rejections` table; opening a schema 1 or 2 database migrates it.
