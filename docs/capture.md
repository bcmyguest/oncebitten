# Signal capture

`oncebitten capture --host <host>` reads one hook payload from stdin, derives signals with no model call and no network, and records them in SQLite.

- Database: `<data dir>/oncebitten/events.db`. The data dir is `$XDG_DATA_HOME` when absolute, else `~/.local/share` on Unix, else `%LOCALAPPDATA%` on Windows, falling back to `<home>\AppData\Local`.
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

`capture` tolerates these field names: session `session_id`, `sessionId`, `sessionID`, `conversation_id`; event `hook_event_name`, `hookEventName`; prompt a string or `{text}`; working directory `cwd`, `directory` or the first of `workspace_roots`. A host that lacks a field gives `None`, never a guess: an event without a prompt text or tool name is an error, and a host that reports no tool outcome records no tool events.

| Host | Status | Fixtures (`tests/fixtures/`) | Records |
| --- | --- | --- | --- |
| claude | verified from real payloads | none committed; field names match the binary | prompts, tool ok, tool failure |
| codex | verified from real payloads (`codex exec`) | `codex/UserPromptSubmit.json`, `codex/PreToolUse.json`, `codex/PostToolUse.json` | prompts only: `PostToolUse` carries `tool_response` as text with no error flag or exit code |
| cursor | `sessionStart` verified from a real `cursor-agent` run; prompt and tool events from docs only (the probe model never reached them) | `cursor/sessionStart.json` | prompts, tool ok, tool failure |
| copilot | `sessionStart` and `userPromptSubmitted` verified from a real `copilot` run; tool events from docs only | `copilot/sessionStart.json`, `copilot/userPromptSubmitted.json` | prompts, tool ok, tool failure. Native payloads carry no event name, so `toolName`/`toolResult` fields tell the events apart. |
| opencode | `session.hook("prompt")` verified from a real `opencode run`; tool hook payload not captured | `opencode/prompt.json` | prompts only |
| pi | verified from docs only (https://pi.dev/docs/latest/extensions); pi is not installed here | none | prompts, tool ok, tool failure |

Codex, cursor and copilot fixtures hold real payloads with personal paths and the account email replaced. Codex and cursor payloads add no repo field beyond `cwd` / `workspace_roots`.

## Install

```sh
oncebitten hooks install --host <claude|codex|cursor|copilot|opencode|pi> --scope <user|project>
oncebitten hooks uninstall --host <host> --scope <user|project>
```

| Host | File (`<home>` / `.`) | Layout |
| --- | --- | --- |
| claude | `.claude/settings.json` | `UserPromptSubmit`, `PostToolUse`, `PostToolUseFailure` merged into `hooks` |
| codex | `.codex/hooks.json` | `UserPromptSubmit` merged into `hooks`. Codex may require trusting the hook on first run. |
| cursor | `.cursor/hooks.json` | `beforeSubmitPrompt`, `postToolUse`, `postToolUseFailure`, flat `{command}` entries, `version` 1 |
| copilot | user `.copilot/hooks/oncebitten.json`, project `.github/hooks/oncebitten.json` | file owned by oncebitten: `userPromptSubmitted`, `postToolUse` with `bash` and `powershell` commands |
| opencode | user `.config/opencode/plugins/oncebitten.js`, project `.opencode/plugins/oncebitten.js` | JS plugin owned by oncebitten |
| pi | user `.pi/agent/extensions/oncebitten.js`, project `.pi/extensions/oncebitten.js` | JS extension owned by oncebitten |

- `<home>` is `HOME`, or on Windows `USERPROFILE`, then `HOMEDRIVE`+`HOMEPATH`.
- Commands call this executable by absolute, canonicalized path (`std::env::current_exe`), never through `PATH`. The path is single-quoted on Unix, double-quoted on Windows (cmd), `& '...'` in the copilot `powershell` field. No `${VAR:-default}` syntax. Running install again adds nothing; after the binary moves, it rewrites the path in place.
- Shared files (claude, codex, cursor) keep other keys and hooks; uninstall removes only oncebitten's entries. Owned files (copilot, opencode, pi) are written whole and deleted on uninstall. Both refuse a file they did not write.
- The opencode and pi scripts spawn the command with a shell, forward the event JSON on stdin, ignore every error and skip when `ONCEBITTEN_TRIAGE=1`.

## Plugin packaging

`packaging/<host>/` holds plugin manifests and hook files for claude, codex, cursor and copilot, plus the opencode and pi scripts that `hooks install` writes. The plugin hooks run `"${<ROOT>}/bin/oncebitten" capture --host <host>`, with `<ROOT>` the host's plugin-root variable (`CLAUDE_PLUGIN_ROOT`, `PLUGIN_ROOT` for codex and copilot, `CURSOR_PLUGIN_ROOT`). Place the `oncebitten` binary in the plugin's `bin/` directory. The plugin files are not tested against a live host install.
