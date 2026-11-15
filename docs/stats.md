# Stats

`oncebitten stats` prints the funnel from captured signals to rules: how many
signals each mistake class produced, and where its clusters stopped. It is
read-only.

```
oncebitten stats [--since <days>] [--json]
```

- `--since <days>`: count only records from the last N days.
- `--json`: print the report as JSON.

With no evidence store yet, it prints `no data: ...` and exits 0. It never creates
the store (`oncebitten doctor` reports the same path).

## Stages

Each mistake class (`failure`, `test-fail-then-pass`, `revert`, `user-correction`)
gets one block:

| Stage | Counts | Source |
| --- | --- | --- |
| signals captured | signal rows of the class's kinds | signals table |
| clusters at threshold | clusters whose sessions meet the minimum | evidence clustering |
| triage runs | n/a | not recorded (see below) |
| rule-shaped drafts | triage answers that drafted a rule | proposals queue |
| proven | drafts whose replay proof stands | proposals queue |
| accepted | drafts accepted | decisions record |
| rejected | drafts rejected | proposals queue |
| waiting | drafts not yet decided | proposals queue |

Rejecting a cluster or accepting one deletes its signal rows, so the signals count
drops after a decision. `--since` filters on record time.

`triage runs` prints `n/a`. The store writes a queue row only when triage answers.
A triage run that fails or returns a finding without a draft leaves no row, so the
count cannot come from the store.

## JSON

```json
{
  "since_days": null,
  "kinds": [
    {
      "kind": "failure",
      "signals_captured": 2,
      "clusters_at_threshold": 1,
      "triage_runs": null,
      "drafts": 1,
      "proven": 0,
      "accepted": 0,
      "rejected": 0,
      "waiting": 1
    }
  ]
}
```

`triage_runs` is `null` where the stage is not recorded. Each kind row has the
same keys.
