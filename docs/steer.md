# Steer rules

Git enforces block rules. Some lessons leave no trace in git: running `sudo`, `git push --force` to main, editing a generated file. A steer rule covers them. It watches the agent's tool calls and prompts through the agent's own hooks and either adds its message to the agent's context (`inform`) or refuses the tool call (`deny`). A prompt rule can hold its message until the agent tries to finish the turn (`at = "stop"`).

`check`, `gate` and `baseline update` never run a steer rule. `gate pr` still requires a valid proof for each active steer rule.

## Rule format

```toml
id = "no-sudo"
description = "Agents never run sudo; the user runs privileged commands."
mode = "steer"
status = "active"
engine = "steer"

[steer]
action = "deny"
message = "Do not run sudo. Print the command and ask the user to run it."

[steer.trigger]
tool = "{bash,shell,run_shell_command}"
command = '(^|[;&|(\n])\s*(env\s+(-\S+\s+|\w+=\S*\s+)*|command\s+|exec\s+|nohup\s+|time\s+)?(\S*/)?sudo($|[\s;&|)])'
```

| Key | Required | Values |
|---|---|---|
| `mode` | yes | `steer`; `engine = "steer"` needs it |
| `engine` | yes | `steer` |
| `[steer] action` | no | `inform` (default) or `deny`. `deny` needs a `tool` trigger. |
| `[steer] at` | no | `prompt` (default) or `stop`. `stop` needs a `prompt` trigger and `action = "inform"`; see [At stop](#at-stop). |
| `[steer] message` | yes | non-empty text, sent as `oncebitten <id>: <message>` |
| `[steer.trigger]` | yes | exactly one of the three triggers below |

Triggers:

| Trigger | Matches |
|---|---|
| `tool = "<glob>"`, optional `command = "<regex>"` | a tool call whose tool name matches the glob (case-insensitive) and, with `command`, whose shell command matches the regex |
| `paths = ["<glob>", ...]` | an edit or write tool call touching a file that matches a glob. Paths are repo-relative with `/` separators; `*` stays within one directory, `**` crosses directories. Read tools never match. |
| `prompt = "<regex>"` | a prompt the user submits |

Tool names differ per host: Claude Code and Codex say `Bash`, Cursor `Shell`, opencode `shell`, pi and Copilot `bash`. A glob such as `{bash,shell}` covers them all. Regexes use Rust `regex` syntax.

Edit paths come from `file_path`, `filePath`, `path` or `notebook_path` in the tool input. Codex `apply_patch` and opencode `patch` carry patch text instead; the paths are read from its `*** Add File:`, `*** Update File:`, `*** Delete File:` and `*** Move to:` lines.

## At stop

Some lessons go wrong at the end of a turn: "address every review comment before saying done". A prompt rule with `at = "stop"` delivers its message when the agent tries to finish the turn that the prompt started.

```toml
[steer]
at = "stop"
message = "Before you finish, check that every review comment is addressed."

[steer.trigger]
prompt = '(?i)review comments?'
```

1. On a matching prompt the hook prints nothing and records `(session, rule)` in the `steer_pending` table of the evidence store (`session TEXT, rule TEXT, ts INTEGER`, primary key `(session, rule)`). A rule is recorded once per session; rows accumulate until the stop.
2. On the host's `Stop` event the hook reads the session's rows, deletes them all, drops each rule that is no longer active, and prints `{"decision": "block", "reason": "oncebitten <id>: <message>"}` with one line per rule. The host keeps the agent going with the reason as its instruction.
3. The next `Stop` of that turn carries `stop_hook_active: true`. The hook then prints nothing and deletes the session's rows, so a rule holds the agent back once per turn.

Rows that never meet a stop expire with the events (`evidence.ttl_days`). The hook opens the store only for a prompt that matches an `at = "stop"` rule and for a `Stop` event; a `Stop` without a store prints nothing and creates none. A store error goes to stderr and the hook prints nothing.

`at = "stop"` works on claude and codex. On cursor, copilot, opencode and pi such a rule does nothing: no message with the prompt, none at the stop.

The fixtures of an `at = "stop"` rule are prompt events, as for any prompt rule.

## Fixtures and proofs

A steer rule's fixtures are recorded events in the normalized shape:

```json
{"event": "tool", "tool": "Bash", "command": "sudo apt-get install -y jq"}
{"event": "tool", "tool": "Write", "paths": ["src/gen/api.rs"]}
{"event": "prompt", "prompt": "deploy to production"}
```

`fixtures/bad.json` must trigger the rule; `fixtures/fixed.json` and every `corpus/*.json` must not. `oncebitten replay <id>` checks this with the matcher in the running binary. The matcher reads only the fixture files, so the replay needs no sandbox; `--local` and `--gateway` are ignored. A proven replay writes `proof.json` with `"backend": "matcher"` and `checker_sha256` naming the `oncebitten` binary. Commit it. `gate pr` and `doctor` replay the fixtures again with their own matcher and fail when it does not prove. The recorded hash is not compared, so a proof from another build of `oncebitten` stays accepted while the replay proves.

## Drafted by triage

Triage drafts a steer rule when a cluster's evidence is a command, or when a user corrected a command the agent ran. It writes `action = "inform"` and fixtures built from the recorded command (`bad`) and the same repo's other successful commands (corpus). Triage proves the draft with the matcher when it queues it. `oncebitten proposals accept <id> --deny` changes the action to `deny` and re-proves the rule. See [proposals.md](proposals.md#steer-drafts).

For agent behaviour that no command and no check on changed files catches, triage drafts a prompt rule with `at = "stop"` and `action = "inform"`. The model writes the prompt regex, the message and the prompt-event fixtures; triage accepts the draft only when the regex matches `bad`, misses `fixed` and every corpus prompt, and does not match the empty string. `accept --deny` refuses such a rule. See [proposals.md](proposals.md#prompt-steer-drafts).

## The steer hook

```sh
oncebitten steer --host <claude|codex|cursor|copilot|opencode|pi>
```

The hook reads the host's payload from stdin, maps a pre-tool or prompt event to the normalized shape, loads rules as `check` does (project, sources, examples) from the payload's working directory, matches every active steer rule and prints the host's output. On claude and codex it also answers `Stop` (see [At stop](#at-stop)) and `SessionStart` (see [Session-start notice](#session-start-notice)). It prints nothing when no rule matches or the event is of another kind. It always exits 0: a bad payload goes to stderr and the agent carries on unsteered. A rule that fails to load disables only itself: its error goes to stderr and the valid rules still match. A clash between valid rules (duplicate id, bad override) still fails the whole load and the agent carries on unsteered. With `ONCEBITTEN_TRIAGE=1` it drains stdin and exits 0.

`oncebitten hooks install` writes the steer hook beside the capture hook for the same host and scope, and `hooks uninstall` removes both. The claude and codex plugin packages in `packaging/` install the steer hook on `UserPromptSubmit`, `PreToolUse`, `Stop` and `SessionStart`, beside the capture hook. The cursor and copilot plugins install only the capture hook. While a plugin's release binary is still downloading, its launcher drops the steer event and exits 0.

## Session-start notice

On `SessionStart` the hook looks for rule proposals (kind `proposal`, state `waiting`) queued for the repository of the payload's `cwd`. When any wait, it prints:

```json
{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "oncebitten: 2 rule suggestion(s) wait for review in this repository. Tell the user, show each with `oncebitten proposals show <id>`, and ask whether to accept or reject it; apply the answer with `oncebitten proposals accept <id>` or `oncebitten proposals reject <id> --reason <text>`. Ids: <id>, <id>"}}
```

Findings do not count. With no waiting proposal, no store, or an error, the hook prints nothing; it never creates the store. The host sends `SessionStart` on startup, resume, clear and compaction, and the notice repeats on each. It works on claude and codex. See [proposals.md](proposals.md#session-start-notice).

## Hosts

| Host | Hook | Deny a tool call | Inform before a tool call | Inform on a prompt | Inform at stop (`at = "stop"`) | Session-start notice | Verified |
|---|---|---|---|---|---|---|---|
| claude | `PreToolUse`, `UserPromptSubmit`, `Stop`, `SessionStart` in `.claude/settings.json` | `hookSpecificOutput.permissionDecision = "deny"` with `permissionDecisionReason` | `hookSpecificOutput.additionalContext` | `hookSpecificOutput.additionalContext` | `{"decision": "block", "reason"}` | `hookSpecificOutput.additionalContext` | deny, by a real `claude -p` run; `scripts/e2e-live.sh` repeats it with the shipped `no-sudo` rule. Stop and session start: from docs. |
| codex | `PreToolUse`, `UserPromptSubmit`, `Stop`, `SessionStart` in `.codex/hooks.json` | as claude | as claude | as claude | as claude; Codex sends the reason as a new user prompt | as claude | deny, by a real `codex exec` run. Stop and session start: from docs. Codex runs a new hook only after you trust it (or with `--dangerously-bypass-hook-trust`). |
| cursor | `preToolUse` in `.cursor/hooks.json` | `{"permission": "deny", "user_message", "agent_message"}` | no | no | no | no | from docs |
| copilot | `preToolUse` in `oncebitten-steer.json` beside the capture file | `{"permissionDecision": "deny", "permissionDecisionReason"}` | no | no | no | no | from docs |
| opencode | `oncebitten-steer.js` plugin beside the capture plugin: `tool.hook("execute.before")`, `session.hook("prompt")` | the plugin throws, which fails the tool call | no | appends the message to the prompt text | no | no | deny, by a real `opencode run` |
| pi | `oncebitten-steer.js` extension beside the capture extension: `tool_call`, `before_agent_start` | `{block: true, reason}` | late: `pi.sendMessage(..., {deliverAs: "steer"})` lands after the turn's tool calls run | `before_agent_start` returns `message` | no | no | script driven by a stub `pi` object; no live run |

Where a cell says no, the host's hooks have no field for it or oncebitten installs no hook on that event. `hooks install` prints a `note:` line for cursor, copilot, opencode and pi naming what does not work. An `inform` rule does nothing on cursor and copilot. An `at = "stop"` rule does nothing on cursor, copilot, opencode and pi.

- Cursor `beforeSubmitPrompt` can only allow or block, and only `sessionStart`, `postToolUse` and `postToolUseFailure` take `additional_context`, so no steer hook is installed for prompts.
- Copilot drops the output of file-based `userPromptSubmitted` hooks, and its file-based `preToolUse` has no `additionalContext` (only the SDK's `onPreToolUse` has it). A copilot hook that exits non-zero denies the call; `oncebitten steer` always exits 0.
- The opencode and pi scripts run `oncebitten steer` with a 10 s timeout and ignore every error. The binary's output for these hosts is `{"deny": "...", "context": "..."}`, which only the scripts read.

Sources:

- Claude Code: <https://code.claude.com/docs/en/hooks> ("Stop decision control": `decision: "block"` with `reason`; "Stop input": `stop_hook_active`; "SessionStart decision control": `additionalContext`)
- Codex: <https://developers.openai.com/codex/hooks> ("Stop": `stop_hook_active`, `decision: "block"` with `reason`, JSON only on stdout; "SessionStart": `hookSpecificOutput.additionalContext`)
- Cursor: <https://cursor.com/docs/agent/hooks>
- Copilot: <https://docs.github.com/en/copilot/reference/hooks-reference>
- opencode v2.2.5: the plugin adapter in the opencode binary (`ctx.tool.hook`, `ctx.session.hook`); the v1 `tool.execute.before` docs on opencode.ai do not apply.
- pi 0.73.1: `docs/extensions.md` and `dist/core/extensions/types.d.ts` in the `@mariozechner/pi-coding-agent` package.

## Example

`examples/no-sudo` ships inactive (`status = "proposed"`). To use it, list it in the project config:

```toml
[rules]
use_examples = ["no-sudo"]
```

A listed example is active and resolves as `examples/no-sudo`.
