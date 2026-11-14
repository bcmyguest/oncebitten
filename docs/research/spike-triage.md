# Spike: headless triage calls per agent host

Scope: PLAN.md step 1, host contract (step 8). Five requirements per host: (a) JSON matching a schema, (b) hooks/plugins disabled, (c) no tools or read-only, (d) timeout, (e) model choice or host default.

Test prompt: classify `user: no, stop using python3 -c, I told you to use a notebook` as `{"repeatable", "user_correction", "root_cause"}`.
Scratch files: `/tmp/spike-triage` (schema.json, raw outputs, sampler logs). Real model calls: claude 3, codex 3, cursor 1 (failed), opencode 3 (2 hung).
Hook verification method: stream events (Claude `--include-hook-events`) and sampling `pgrep` for `hook-runner.mjs` (Codex) during each call.

## Summary

| Host | Version | Schema (a) | Hooks off (b) | Read-only (c) | Timeout (d) | Model (e) | Status |
|---|---|---|---|---|---|---|---|
| claude | 2.1.325 | verified | verified | verified | `timeout 120` (not per-flag) | verified | usable |
| codex | 0.167.2 | verified | verified | flag accepted, not behaviourally verified | `timeout 120` | verified (`-m`) | usable |
| cursor-agent | 2026.10.22 | none (prompt only) | none found, unverified | `--mode ask` documented, unverified | `timeout 120` | `--model`, unverified | blocked: usage limit, resets 2026-11-18 |
| opencode | 2.2.5 | none (prompt only, fenced JSON) | none found, unverified | `--agent plan`, unverified | unreliable: process hangs after output | `-m provider/model`, default from config | usable with caveats |
| pi | not installed | - | - | - | - | - | not tested |

## claude (2.1.325)

- Command (verified, exit 0, 3.4 to 4.0 s wall):
  `claude -p "$P" --output-format json --setting-sources project,local --disable-slash-commands --tools "" --json-schema "$(cat schema.json)" --model haiku --no-session-persistence </dev/null`
- Schema: verified. Result entry carries `structured_output`, which matched the schema. Claude enforces it via an internal `StructuredOutput` tool.
- Hooks off: partly verified, user-level hooks only. `--setting-sources project,local` still loads project hooks, and the test directory had none. Default run with `--include-hook-events`: 10 `hook_started` events (SessionStart x8, UserPromptSubmit x8, Stop x2, PostToolUse:Skill x2 across the stream). Same prompt with `--setting-sources project,local`: 0 hook events. `--bare` also skips hooks but reads no keychain, so auth risk; not tested.
- Read-only: `--tools ""` verified. The only tool call was `StructuredOutput`; `permission_denials` empty.
- Model: `--model haiku` verified (`modelUsage` shows haiku). Omit the flag for host default; not separately tested.
- Latency: 3.4 s and 4.0 s wall with hooks off; 7.9 s with hooks on. Cost about $0.001 per call.
- Failure modes: not tested (auth missing, not installed).

## codex (0.167.2)

- Command (verified, exit 0, about 3.7 s wall):
  `codex exec --skip-git-repo-check --json --ephemeral --disable hooks -s read-only -m gpt-6.1-terra --output-schema schema.json -o out.json "$P" </dev/null`
  The verified run omitted `--disable hooks`; the flag was verified on its own (below). The combined command was not rerun.
- Schema: verified. `-o` file matched the schema. `--json` emits JSONL events on stdout; the answer is in `-o`.
- Hooks off: verified. `--disable hooks` (feature flag, per invocation). Sampling found 3 `hook-runner.mjs codex` processes in the baseline (sessionStart, prompt, stop) and 0 with `--disable hooks`. Baseline also warns about hooks loaded from both `hooks.json` and `config.toml`.
- Read-only: `-s read-only` accepted; no behavioural test.
- Model: `-m` verified with the config default `gpt-6.1-terra`. Other model names not tested.
- Failure modes: outside a git repo, exits 1 unless `--skip-git-repo-check` is set. Without `</dev/null`, it waits on stdin. Baseline input was 17,934 tokens for a one-line prompt, which costs more than the other hosts.

## cursor-agent (2026.10.22)

- Command (run, exit 1): `agent -p --output-format json --mode ask --trust --workspace /tmp/spike-triage "$P" </dev/null`
- Failure verified: `ActionRequiredError: You've hit your usage limit`. Resets 2026-11-18. No model output, so every other criterion is unverified.
- Schema: no schema flag in `--help`. Prompt-only.
- Hooks off: no per-invocation flag in `--help`. User hooks live in `~/.cursor/hooks.json` (sessionStart, beforeSubmitPrompt, and others). Unverified.
- Read-only: `--mode ask` ("read-only" per help). Unverified.
- Model: `--model <name>`; `--list-models` exists. Unverified.

## opencode (2.2.5)

- Command (run, exit 124 after output): `opencode run --format json --agent plan "$P" </dev/null`
- Output: JSONL events. Text arrives in a `text` part wrapped in a ```json fence. Strip the fence and parse. The answer parsed correctly.
- Schema: none. Prompt-only.
- Hooks off: no verified mechanism. Plugins load from `~/.config/opencode/plugin/` (ski.ts) and `plugins/` (rtk.ts, herdr). Env vars such as `OPENCODE_CONFIG_DIR` and `OPENCODE_DISABLE_PROJECT_CONFIG` exist; none verified to disable plugins.
- Read-only: `--agent plan`. Unverified.
- Model: `-m provider/model`. Config default `lemonade/Qwen3.8-Flash-Next-GGUF-UD-IQ4_XS`.
- Timeout: unreliable. Run 1 emitted its answer in 5.0 s, then hung until `timeout 120` killed it. Run 2 hung for 60 s with no output. Run 3 with `--standalone` ended with `ECONNRESET` after the background move. The triage caller must bound the call and parse output before exit.

## pi

- Not installed: `command -v pi` returns nothing. Not tested.
- Reference adapter `/home/b/wteam/src/adapters/pi.rs` uses `--mode json --model <m> -- <prompt>`. Unverified.

## Open items

- Hook disable for cursor-agent and opencode needs a call after the usage limit resets, and a plugin-load log check for opencode.
- Claude `--bare` auth behaviour is untested.
