# Signal capture

`oncebitten capture --host <host>` reads one hook payload from stdin, derives signals with no model call and no network, and records them in SQLite.

- Database: `<data dir>/oncebitten/events.db`. The data dir is `$XDG_DATA_HOME` when absolute, else `~/.local/share` on Unix, else `%LOCALAPPDATA%` on Windows, falling back to `<home>\AppData\Local`.
- Exits 0 on every path, prints nothing to stdout, writes one line to stderr on error.
- `ONCEBITTEN_TRIAGE=1` makes it exit 0 without reading stdin or writing.
- An event that brings a cluster to its threshold starts `oncebitten triage <id> --queue` detached, unless `triage.background = false`. Capture does not wait for it, and a failure to start it is one stderr line, not a failed hook. See [proposals.md](proposals.md#background-triage).
- 100 captures finish in under 5 s (test `one_hundred_captures_finish_in_under_five_seconds`).
- A hook payload with a `tool_use_id` is one event: a redelivery of the same id stores nothing. Without one, the event is deduplicated on its whole content, and a prompt is also deduplicated against the same prompt in the same session within 2000 ms. See [evidence.md](evidence.md#provenance-and-dedupe).

## Signals

| Kind | Fires when |
| --- | --- |
| `tool-failure` | A tool run reports failure (`PostToolUseFailure`). |
| `repeat-failure` | A second failure with the same tool and normalized command in one session. Normalization collapses whitespace and replaces digits, paths and hex with `<n>`, `<path>` and `<hex>`. |
| `test-fail-then-pass` | A test runner (`cargo test`, `pytest`, `npm test`, `vitest`, `jest`, `go test`, or a `[capture] test_commands` entry) passes after the session's previous test run failed. |
| `user-correction` | A prompt contains a phrase from the correction lexicon. Its detail lists the matched phrases, comma-separated and lowercased. |
| `revert` | A shell-tool `git revert` (Bash, Shell, shell, bash), or `git checkout --` / `git restore` of a file the session edited. Edited and command paths match when one is a path-component suffix of the other. |

### Hosts and signals

Tool names match without regard to case, so `Bash`, `Shell`, `shell` and `bash` all run commands, and `Edit`, `Write`, `MultiEdit`, `edit` and `write` all edit files. `revert` and `test-fail-then-pass` fire the same way on claude, cursor, copilot and pi. codex and opencode report no tool outcomes (`Host::capabilities` in `src/host.rs`), so capture records only their prompts and they produce only `user-correction`. `oncebitten doctor` prints this per installed host, derived from the same capabilities.

### Test commands

`[capture] test_commands` adds command prefixes to the built-in runners. A command counts as a test run when it contains an entry, matched without regard to case and only at word boundaries, like the built-ins. Blank entries fail the load.

```toml
[capture]
test_commands = ["make check", "just test"]
```

### Correction lexicon

- Default list: `DEFAULT_CORRECTION_LEXICON` in `src/signal.rs`. It holds corrections (`no,`, `that's wrong`), frustration (`again`, `you keep`) and stated rules (`from now on`, `make sure`).
- `[capture] correction_phrases` replaces the default list. `[capture] extra_correction_phrases` adds to the list in effect. Both take a TOML array. Set either in the user or project config file (see `docs/configuration.md`).
- Matching ignores case and treats `’` as `'`. A phrase matches only where it is not inside a longer word: `no,` does not match in `know,`.
- A phrase matches once per prompt, however often it occurs. The list is searched in order, so the detail order follows the list.

## Privacy

- A prompt that raises a `user-correction` signal is stored as text, cut to 2000 characters, in `events.summary`. Triage needs the text to draft a rule from what the user said. Any other prompt is stored as `sha256:<hex>` of its text, never the text. The `user-correction` detail holds only the matched phrases.
- Schema 2 (`PRAGMA user_version = 2`) migrated schema 1 databases; schema 3 adds the `rejections` table (see [evidence.md](evidence.md)); schema 4 (`PRAGMA user_version = 4`) adds `events.git_head`. Opening a schema 0 to 3 database migrates it. On first open, each stored schema 1 prompt becomes its digest, corrections included, and each `user-correction` detail is rebuilt from the default lexicon. A phrase that appears only in a custom list is not recovered from old rows.
- The migration overwrites the old rows, then runs `VACUUM` and a `wal_checkpoint(TRUNCATE)`, so no old prompt text stays in freed pages or the WAL. It sets the schema version only after both succeed; if one fails, the next open retries and skips rows already digested.
- Tool summaries (the Bash command or the file path, cut to 2000 characters) are not redacted. The agent writes them from its tool input. A command can still contain text the user typed, for example a pasted value in `echo`. Redacting them would drop the command that the `repeat-failure`, `test-fail-then-pass` and `revert` signals need.

## Hosts

`capture` tolerates these field names: session `session_id`, `sessionId`, `sessionID`, `conversation_id`; event `hook_event_name`, `hookEventName`; prompt a string or `{text}`; working directory `cwd`, `directory` or the first of `workspace_roots`. A host that lacks a field gives `None`, never a guess: an event without a prompt text or tool name is an error, and a host that reports no tool outcome records no tool events.

| Host | Status | Fixtures (`tests/fixtures/`) | Records |
| --- | --- | --- | --- |
| claude | verified from real payloads | none committed; field names match the binary | prompts, tool ok, tool failure |
| codex | verified from real payloads (`codex exec`) | `codex/UserPromptSubmit.json`, `codex/PreToolUse.json`, `codex/PostToolUse.json` | prompts only: `PostToolUse` carries `tool_response` as text with no error flag or exit code |
| cursor | `sessionStart` verified from a real `cursor-agent` run; prompt and tool events from docs only (the probe model never reached them) | `cursor/sessionStart.json` | prompts, tool ok, tool failure |
| copilot | `sessionStart` and `userPromptSubmitted` verified from a real `copilot` run; tool events from docs only | `copilot/sessionStart.json`, `copilot/userPromptSubmitted.json` | prompts, tool ok, tool failure. Native payloads carry no event name, so `toolName`/`toolResult` fields tell the events apart. |
| opencode | `session.hook("prompt")` verified from a real `opencode run`; tool hook payload not captured | `opencode/prompt.json` | prompts only. Accepted as a triage host without a hooks-off proof (user decision); see [triage.md](triage.md). |
| pi | `before_agent_start` (prompt) and `tool_result` (tool ok, tool failure) verified from a real `pi -p` run against a local mock provider (pi 0.73.1, `@mariozechner/pi-coding-agent`); the user-scope path is verified from pi's source only | `pi/before_agent_start.json`, `pi/tool_result.json` | prompts, tool ok, tool failure |

Codex, cursor, copilot and pi fixtures hold real payloads with personal paths and the account email replaced. Codex and cursor payloads add no repo field beyond `cwd` / `workspace_roots`. Pi's session id is a random UUID and is kept. The pi fixtures are the payloads `packaging/pi/oncebitten.js` forwarded during the real run; the raw pi events were not recorded.

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
| pi | user `.pi/agent/extensions/oncebitten.js`, project `.pi/extensions/oncebitten.js` | JS extension owned by oncebitten. Project scope is relative to the directory pi starts in, not the git root, and was verified by a real run. User scope is `~/.pi/agent` by pi's source. When `$PI_CODING_AGENT_DIR` is non-empty, `hooks install` and `hooks uninstall` use `$PI_CODING_AGENT_DIR/extensions/oncebitten.js` instead, expanding a leading `~` as pi does; a relative value is an error. |

- `<home>` is `HOME`, or on Windows `USERPROFILE`, then `HOMEDRIVE`+`HOMEPATH`.
- User-scope commands call this executable by absolute, canonicalized path (`std::env::current_exe`), never through `PATH`. Project-scope installs (`.claude/settings.json` and the other project files) write the bare command `oncebitten ...`, resolved from `PATH`, so the committed file works on every machine;. The absolute path is single-quoted on Unix, double-quoted on Windows (cmd), `& '...'` in the copilot `powershell` field. No `${VAR:-default}` syntax. Running install again adds nothing; after the binary moves, it rewrites the path in place.
- Shared files (claude, codex, cursor) keep other keys and hooks; uninstall removes only oncebitten's entries. Owned files (copilot, opencode, pi) are written whole and deleted on uninstall. Both refuse a file they did not write.
- `hooks install` also writes the steer hook (`oncebitten steer --host <host>`) for the same host and scope, and `hooks uninstall` removes it; see [steer.md](steer.md) for its events, files and what each host cannot do.
- The opencode and pi scripts spawn the command with a shell, forward the event JSON on stdin, ignore every error and skip when `ONCEBITTEN_TRIAGE=1`.

## Plugin packaging

`packaging/<host>/` holds plugin manifests and hook files for claude, codex, cursor and copilot, plus the opencode and pi scripts that `hooks install` writes. The plugin hooks run `"${<ROOT>}/bin/oncebitten" capture --host <host>`, with `<ROOT>` the host's plugin-root variable (`CLAUDE_PLUGIN_ROOT`, `PLUGIN_ROOT` for codex and copilot, `CURSOR_PLUGIN_ROOT`). The claude and codex plugins also run `steer --host <host>`; see [steer.md](steer.md). For every plugin, `bin/oncebitten` is a POSIX `sh` launcher that runs the release binary matching the plugin's version, installing it with the bundled `scripts/install.sh` when missing (see [install.md](install.md)). The plugin's `SessionStart` hook runs it once with `--version` (120 s timeout) so the download happens before any capture. A `capture` call that finds no binary drains stdin, starts the download in the background and exits 0, so that event is dropped rather than delaying the agent. On Windows the launcher needs Git Bash. The codex, cursor and copilot plugins are not tested against a live host install.
