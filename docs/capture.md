# Signal capture

`oncebitten capture --host <host>` reads one hook payload from stdin, derives signals with no model call and no network, and records them in SQLite.

- Database: `<data dir>/oncebitten/events.db`. The data dir is `$XDG_DATA_HOME` when absolute, else `~/.local/share` on Unix, else `%LOCALAPPDATA%` on Windows.
- Exits 0 on every path, prints nothing to stdout, writes one line to stderr on error.
- `ONCEBITTEN_TRIAGE=1` makes it exit 0 without reading stdin or writing.
- 100 captures finish in under 5 s (test `one_hundred_captures_finish_in_under_five_seconds`).

## Signals

| Kind | Fires when |
| --- | --- |
| `tool-failure` | A tool run reports failure (`PostToolUseFailure`). |
| `repeat-failure` | A second failure with the same tool and normalized command in one session. Normalization collapses whitespace and replaces digits, paths and hex with `<n>`, `<path>` and `<hex>`. |
| `test-fail-then-pass` | A test runner (`cargo test`, `pytest`, `npm test`, `vitest`, `jest`, `go test`) passes after the session's previous test run failed. |
| `user-correction` | A prompt contains a phrase from the correction lexicon, case-insensitive (`src/signal.rs`, `CORRECTION_LEXICON`). |
| `revert` | A Bash `git revert`, or `git checkout --` / `git restore` of a file the session edited. |

## Hosts

| Host | Capture | Evidence |
| --- | --- | --- |
| claude | verified | Hook payloads captured from Claude Code (`UserPromptSubmit`, `PostToolUse`, `PostToolUseFailure`). Field names match the binary. |
| codex | unverified, refused | Only the hook config was written. No payload was captured. |
| opencode | unverified, refused | Hooks are plugins, not stdin payloads. No verified mechanism. |
| cursor | unverified, refused | Only the hook config was written (`beforeSubmitPrompt`, `postToolUse`, `postToolUseFailure`). No payload was captured. |
| pi | unverified, refused | Not installed on this machine. |

`capture --host <unverified>` exits 0 and writes `unsupported host` to stderr.

## Install

```sh
oncebitten hooks install --host claude --scope user
oncebitten hooks install --host claude --scope project
oncebitten hooks uninstall --host claude --scope user
```

- User scope edits `$HOME/.claude/settings.json`. Project scope edits `./.claude/settings.json`.
- Install adds one `oncebitten capture --host claude` command to `UserPromptSubmit`, `PostToolUse` and `PostToolUseFailure`. Running it again adds nothing.
- Other keys and hooks stay as they are. Uninstall removes only the entries with that command.
- Only `claude` is accepted.
