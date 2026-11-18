# Triage

`oncebitten triage <cluster-id> [--host <host>]` asks a headless agent host for a one-shot JSON verdict on one evidence cluster (see [evidence.md](evidence.md)). It prints the verdict to stdout as JSON, then queues it with any rule it drafted (see [proposals.md](proposals.md)). A queueing problem prints a warning to stderr and leaves the exit code at 0.

Run on a cluster queued only as a waiting finding, it asks the host again: an answer with a valid draft replaces the finding with the proposal, and an answer without one prints `<id> stays a finding: the answer drafted no rule; nothing written` to stderr. On a cluster with a queued proposal or a decision it still prints the verdict and then `<id> is already queued; nothing written`.

## Verdict

| Field | Type | Values |
|---|---|---|
| `repeatable` | bool | the mistake recurs |
| `user_correction` | bool | the user corrected the agent |
| `root_cause` | string | 1 to 1000 characters |
| `proposed_fix_kind` | enum | `harness`, `agent-workflow`, `lint`, `test`, `test-practice`, `invariant`, `none` |
| `generality` | enum | `general`, `project`, `one-off` |
| `mistake_class` | string or null | lowercase slug, `^[a-z0-9]+(-[a-z0-9]+)*$`, at most 64 characters |

The host must return these five keys plus an optional `draft`: a proposed rule, or `null`. `mistake_class` is optional too: a missing or `null` value parses as `null`, so answers from older hosts still load. Extra keys and wrong types are rejected. An invalid draft rejects the whole answer.

The prompt carries a context block before the cluster details: the ids and descriptions of the loaded rules, and the past rejection reasons. The block holds at most 30 items and 4 KiB, of which up to 10 items and 1 KiB are held for past rejections, so many rules cannot crowd them out. A rule that fails to load is left out of the block. Proposals use `mistake_class` and the rationale to flag possible duplicates; see [proposals.md](proposals.md#possible-duplicates). An invalid draft rejects the whole answer. Only a rule-shaped verdict keeps its draft; the draft fields are in [proposals.md](proposals.md#draft). The printed verdict omits the draft.

After that block the prompt lists the known mistake classes: each `mistake_class` triage named on another cluster, as `- <slug> (clusters=<n>): <latest root cause>`, most clusters first, at most 15 lines and 2 KiB. The host is told to answer with a listed slug when the cluster is the same mistake, and that such a mistake recurred, so `repeatable` is true. This is how one mistake corrected in different words accumulates: each correction prompt stays its own cluster (see [evidence.md](evidence.md)), and the shared class links them. A cluster's own earlier verdict is not counted.

## Hosts

| Host | Status | Command | Hooks off |
|---|---|---|---|
| claude | implemented | `claude -p --output-format json --setting-sources project,local --settings '{"disableAllHooks": true}' --disable-slash-commands --tools "" --json-schema <schema> --no-session-persistence` | proof required |
| codex | implemented; refused on codex 0.167.2 (see below) | `codex exec --skip-git-repo-check --json --ephemeral --disable hooks -s read-only --output-schema <file> -o <file>` | proof required |
| opencode | implemented | `opencode run --format json --agent plan` | no proof (accepted by decision) |
| pi | unsupported | none | none |

The commands follow [docs/research/spike-triage.md](research/spike-triage.md). The spike verified the claude and codex flags. It did not verify the combined codex command, and it found no verified hooks-off mechanism for opencode. The `--settings` inline setting for claude is an addition the spike did not test; the hooks-off proof checks it.

Pi has no verified headless call. Cursor is not a triage host.

What has run for real, as of 2026-11-16:

| Host | Evidence |
|---|---|
| claude | `scripts/e2e-live.sh` on Linux with Claude Code: two sessions, each corrected once, produce a cluster; background triage queues a proven ast-grep proposal; `proposals accept` activates it; `gate pr` passes the existing code and fails a new copy of the rule's bad fixture. |
| codex | The same script with `codex:<model>`: refused by the hooks-off proof (see [Hooks-off proof](#hooks-off-proof)); `proposals list` reports the failure. |
| opencode | Fake-host tests only. |
| Windows | CI runs `oncebitten triage` and background triage against a fake host installed as `claude.cmd` on `PATH`, with the prompt on stdin (`tests/triage_cli_host.rs`). No real host has run on Windows. |

The model comes from the target: `triage.default` or `triage.<host>` in configuration (`<host>:<agent>`). `default` as the agent omits the model flag.

## Steer drafts

When the cluster's evidence is a command, the draft may use `engine = "steer"` (see [proposals.md](proposals.md#steer-drafts)). A user-correction cluster qualifies when a command in its corrected turns is the mistake: the prompt asks for `proposed_fix_kind = "agent-workflow"` and a steer draft on that command. A command steer is inform-only: triage writes `action = "inform"`, and `oncebitten proposals accept <id> --deny` is the only path to deny. Its fixtures come from the recorded command, so triage does not invent the bad case.

A workflow mistake with no recorded command ("most of my comments were not addressed") can get a prompt steer draft instead. The prompt asks for one when the verdict is repeatable, the mistake is agent behaviour that no check on changed files and no single command catches, and a topic in the user's request predicts it. The model writes the trigger: a case-insensitive regex in the draft's `prompt` key that names the topic of the request preceding the mistake (review comments, feedback) and stays narrow enough that ordinary prompts do not match. It also writes the fixtures, as prompt events, and the message in `rationale`: one imperative sentence telling the agent what to verify before it finishes. The written rule has `action = "inform"` and `at = "stop"` (see [steer.md](steer.md#at-stop)); `accept --deny` refuses it. A workflow mistake with no predicting topic still ends as a finding.

## Call contract

- Each call runs in a fresh empty temp directory, removed afterwards, with `ONCEBITTEN_TRIAGE=1`. Every `oncebitten capture` call exits 0 at once when that variable is `1`.
- The prompt goes to the host on stdin, which then closes. Every host reads its prompt from stdin when the command names none. Argv cannot carry it on Windows: npm installs hosts as `.cmd` wrappers, and Rust refuses a multi-line argument to a batch file.
- The prompt holds at most 12,000 bytes, at most 10 occurrences, and at most 400 bytes per summary. A cut ends with `[truncated]`.
- For a user-correction cluster the prompt also lists the corrected turn of each occurrence: the stored summaries of the session's tool calls between the previous prompt and the correction, as `- ts=<ts> <tool>: <summary> | ...`. It keeps the last 8 calls of a turn, 120 bytes per call and 400 bytes per turn, inside the 12,000-byte limit. Capture stores no agent message text, so the turn holds tool calls only; a turn without tool calls adds no line.
- The default timeout is 120 seconds. A call that exceeds it is killed.
- Opencode may print its answer and keep running. The caller parses the first complete verdict from the JSON event lines and kills the process.
- The model's output must match the schema. Invalid or missing JSON, a timeout, and a missing binary are "could not run".

## Hooks-off proof

Before a claude or codex call, oncebitten runs the same call with hooks on and sentinel hooks installed in the temp directory's project scopes. Each sentinel writes a marker file. The control must fire every sentinel, or the proof is inconclusive. Then it runs the triage call with hooks off and fails if any sentinel fires.

codex 0.167.2 does not run the project-scope sentinel: a `.codex/hooks.json` in the temp directory never fires, with or without `--dangerously-bypass-hook-trust` (run on 2026-11-16). The control therefore fails and every codex triage ends with `hooks-off proof inconclusive: sentinels did not fire with hooks on (codex project hooks)`. Codex is unusable as a triage host until the proof has a codex scope it can fire.

The user settings scope (`~/.claude`, `~/.codex`) is not sentinel-tested. Those files hold the host's credentials and the proof cannot move HOME. Coverage of that scope rests on the hooks-off flag for each host.

## Code hunks

Before the call, triage reads the code the sessions changed. For each occurrence with a stored `git_head`, it diffs that commit against the working tree, limited to the files the session's edit tools named. Codex `apply_patch` and opencode `patch` edits contribute every file their patch names. The diff is appended to the prompt, capped at `triage.max_code_bytes` (default 16,384) on top of the 12,000-byte prompt limit. Nothing from the diff is stored; the store keeps only hashes and phrases.

An ast-grep or native draft's `bad` fixture must contain a run of consecutive lines from one hunk's post-image (context and added lines, in order) that includes at least one substantive added line: a blank line, or one holding only braces, brackets, parentheses, `;` or `,`, does not count. A diff draft's `bad` lists paths and needs no quote. Line endings are normalised to LF and trailing spaces ignored. A draft that quotes no changed code is "could not run".

Code hunks go to the configured triage host. The user is responsible for that host's data handling. To keep code out of the prompt, set `send_code = false` under `[triage]`; triage then runs as before with no code and no quote check. No hunks found (no `git_head`, no edits, or a git failure) also runs as before.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | verdict printed to stdout |
| 2 | could not run: unknown cluster, no target, unsupported host, refused host, proof failed, timeout, missing binary, or invalid output |

No verdict is ever printed with exit 2.

## Background mode

`capture` starts `oncebitten triage <id> --queue` (a hidden flag) when a cluster reaches its threshold. It takes the cluster's triage lock, queues the answer, prints nothing and exits 2 on any failure, after storing the error in a `failed: <error>` row. `oncebitten proposals list` prints each cluster whose latest row is such a failure to stderr, with the host asked and the retry command; a later run of the cluster, or a queued proposal, clears the line. The row's `host` is the host asked, which differs from the `started` row's capturing host when the default target answered for it. See [proposals.md](proposals.md#background-triage).

## Daily budget

Each triage run is two host calls for claude and codex, so `triage.max_per_day` (default `10`) caps background cost. The `triage_runs` table in the events store records every run: `ts` (unix ms), `cluster`, `host` and `outcome`.

| outcome | written by | counts toward the cap |
| --- | --- | --- |
| `started` | `capture` when it spawns `triage --queue`; manual `triage <id>` | yes |
| `skipped: budget` | `capture` when the day's runs reach the cap; spawns nothing | no |
| `spawn-failed` | `capture` when the `triage --queue` child cannot start | no |
| `failed: <error>` | the `triage --queue` child when it exits 2; the error is one line, cut to 300 characters | no; the run's `started` row already counted |

The day is the local calendar day. `capture` counts `started` rows since local midnight, then spawns or skips each due cluster. The `--queue` child writes no `started` row, so one background run counts once. Manual `triage <id>` runs past the cap and prints `warning: triage budget spent: N of M runs today` to stderr; it still counts as a run.

A cluster skipped for budget gets one `skipped: budget` row. The next `capture` in the same repo, once the day's runs fall under the cap, starts it: oldest skip first, and only while its latest row is still the skip. A cluster with a proposal or rejection is never picked up again. `triggers.session` filters background starts in both paths (see [configuration.md](configuration.md)).
