# Triage

`oncebitten triage <cluster-id> [--host <host>]` asks a headless agent host for a one-shot JSON verdict on one evidence cluster (see [evidence.md](evidence.md)). It prints the verdict to stdout as JSON, then queues it with any rule it drafted (see [proposals.md](proposals.md)). A queueing problem prints a warning to stderr and leaves the exit code at 0.

## Verdict

| Field | Type | Values |
|---|---|---|
| `repeatable` | bool | the mistake recurs |
| `user_correction` | bool | the user corrected the agent |
| `root_cause` | string | 1 to 1000 characters |
| `proposed_fix_kind` | enum | `harness`, `agent-workflow`, `lint`, `test`, `test-practice`, `invariant`, `none` |
| `generality` | enum | `general`, `project`, `one-off` |

The host must return these five keys plus an optional `draft`: a proposed rule, or `null`. Extra keys, missing keys, and wrong types are rejected. An invalid draft rejects the whole answer. Only a rule-shaped verdict keeps its draft; the draft fields are in [proposals.md](proposals.md#draft). The printed verdict omits the draft.

## Hosts

| Host | Status | Command | Hooks off |
|---|---|---|---|
| claude | implemented | `claude -p --output-format json --setting-sources project,local --settings '{"disableAllHooks": true}' --disable-slash-commands --tools "" --json-schema <schema> --no-session-persistence` | proof required |
| codex | implemented | `codex exec --skip-git-repo-check --json --ephemeral --disable hooks -s read-only --output-schema <file> -o <file>` | proof required |
| opencode | implemented | `opencode run --format json --agent plan` | no proof (accepted by decision) |
| pi | unsupported | none | none |

The commands follow [docs/research/spike-triage.md](research/spike-triage.md). The spike verified the claude and codex flags. It did not verify the combined codex command, and it found no verified hooks-off mechanism for opencode. The `--settings` inline setting for claude is an addition the spike did not test; the hooks-off proof checks it.

Pi has no verified headless call. Cursor is not a triage host.

The model comes from the target: `triage.default` or `triage.<host>` in configuration (`<host>:<agent>`). `default` as the agent omits the model flag.

## Call contract

- Each call runs in a fresh empty temp directory, removed afterwards, with `ONCEBITTEN_TRIAGE=1`. Every `oncebitten capture` call exits 0 at once when that variable is `1`.
- The prompt goes to the host on stdin, which then closes. Every host reads its prompt from stdin when the command names none. Argv cannot carry it on Windows: npm installs hosts as `.cmd` wrappers, and Rust refuses a multi-line argument to a batch file.
- The prompt holds at most 12,000 bytes, at most 10 occurrences, and at most 400 bytes per summary. A cut ends with `[truncated]`.
- The default timeout is 120 seconds. A call that exceeds it is killed.
- Opencode may print its answer and keep running. The caller parses the first complete verdict from the JSON event lines and kills the process.
- The model's output must match the schema. Invalid or missing JSON, a timeout, and a missing binary are "could not run".

## Hooks-off proof

Before a claude or codex call, oncebitten runs the same call with hooks on and sentinel hooks installed in the temp directory's project scopes. Each sentinel writes a marker file. The control must fire every sentinel, or the proof is inconclusive. Then it runs the triage call with hooks off and fails if any sentinel fires.

The user settings scope (`~/.claude`, `~/.codex`) is not sentinel-tested. Those files hold the host's credentials and the proof cannot move HOME. Coverage of that scope rests on the hooks-off flag for each host.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | verdict printed to stdout |
| 2 | could not run: unknown cluster, no target, unsupported host, refused host, proof failed, timeout, missing binary, or invalid output |

No verdict is ever printed with exit 2.

## Background mode

`capture` starts `oncebitten triage <id> --queue` (a hidden flag) when a cluster reaches its threshold. It takes the cluster's triage lock, queues the answer, prints nothing and exits 2 on any failure. See [proposals.md](proposals.md#background-triage).
