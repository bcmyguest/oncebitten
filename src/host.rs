use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::signal::{KIND_PROMPT, KIND_TOOL_ERROR, KIND_TOOL_OK};
use crate::store::NewEvent;

const GIT_MARKER: &str = ".git";
const COMMAND_TOOLS: &[&str] = &["bash", "shell"];
const PATH_TOOLS: &[&str] = &["edit", "write", "multiedit", "read"];
const SESSION_KEYS: &[&str] = &["session_id", "sessionId", "sessionID", "conversation_id"];
const EVENT_KEYS: &[&str] = &["hook_event_name", "hookEventName"];
const CWD_KEYS: &[&str] = &["cwd", "directory"];
const TOOL_KEYS: &[&str] = &["tool_name", "toolName", "tool"];
const INPUT_KEYS: &[&str] = &["tool_input", "toolInput", "toolArgs", "input"];
const PATH_KEYS: &[&str] = &["file_path", "filePath", "path"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Claude,
    Codex,
    OpenCode,
    Cursor,
    Copilot,
    Pi,
}

/// What a host reports. A `false` field is absent from the host's payload; the
/// matching event is never recorded and its value is never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub prompt: bool,
    /// The host reports a tool call together with its outcome.
    pub tool_outcome: bool,
}

pub const ALL_HOSTS: [Host; 6] = [
    Host::Claude,
    Host::Codex,
    Host::Cursor,
    Host::Copilot,
    Host::OpenCode,
    Host::Pi,
];

impl Host {
    pub fn parse(name: &str) -> Result<Host> {
        ALL_HOSTS
            .into_iter()
            .find(|host| host.name() == name)
            .with_context(|| {
                format!(
                    "unknown host `{name}`; expected claude, codex, cursor, copilot, opencode or pi"
                )
            })
    }

    pub fn name(self) -> &'static str {
        match self {
            Host::Claude => "claude",
            Host::Codex => "codex",
            Host::OpenCode => "opencode",
            Host::Cursor => "cursor",
            Host::Copilot => "copilot",
            Host::Pi => "pi",
        }
    }

    pub fn capabilities(self) -> Capabilities {
        // Codex PostToolUse carries `tool_response` as bare text: no exit code, no
        // error flag. OpenCode's tool hook payload is unverified, so its plugin
        // forwards prompts only. Recording either would invent an outcome.
        let tool_outcome = !matches!(self, Host::Codex | Host::OpenCode);
        Capabilities {
            prompt: true,
            tool_outcome,
        }
    }
}

enum Outcome {
    Prompt,
    ToolOk,
    ToolError,
}

pub fn normalize(host: Host, payload: &str, ts: i64) -> Result<Option<NewEvent>> {
    let value: Value = serde_json::from_str(payload).context("hook payload is not JSON")?;
    let session_id = first_string(&value, SESSION_KEYS).with_context(|| {
        "hook payload has no session id (session_id, sessionId, sessionID or conversation_id)"
    })?;
    let Some(outcome) = classify(&value) else {
        return Ok(None);
    };
    let caps = host.capabilities();
    let (kind, tool, summary) = match outcome {
        Outcome::Prompt if caps.prompt => {
            let prompt = prompt_text(&value).context("hook payload has no prompt text")?;
            (KIND_PROMPT, None, prompt)
        }
        Outcome::ToolOk | Outcome::ToolError if caps.tool_outcome => {
            let tool = first_string(&value, TOOL_KEYS).context("hook payload has no tool name")?;
            let summary = tool_summary(&value, &tool);
            let kind = if matches!(outcome, Outcome::ToolError) {
                KIND_TOOL_ERROR
            } else {
                KIND_TOOL_OK
            };
            (kind, Some(tool), summary)
        }
        _ => return Ok(None),
    };
    Ok(Some(NewEvent {
        ts,
        host: host.name().to_string(),
        session_id,
        repo: cwd(&value).and_then(|dir| find_repo(&dir)),
        kind: kind.to_string(),
        tool,
        summary,
    }))
}

/// Maps a host's event name, or the fields present when it names none, onto an outcome.
/// Unknown events (session start, pre-tool, stop) map to nothing.
fn classify(value: &Value) -> Option<Outcome> {
    let tool_failed = value["isError"] == true
        || value["is_error"] == true
        || matches!(value["status"].as_str(), Some("error"))
        || value["toolResult"]["resultType"].as_str() == Some("failure")
        || value["error"].is_string()
        || value["error"].is_object();
    match first_string(value, EVENT_KEYS)
        .map(|name| name.to_ascii_lowercase())
        .as_deref()
    {
        Some(
            "userpromptsubmit"
            | "userpromptsubmitted"
            | "beforesubmitprompt"
            | "before_agent_start"
            | "prompt",
        ) => Some(Outcome::Prompt),
        Some("posttooluse" | "tool_result" | "tool.execute.after") => Some(if tool_failed {
            Outcome::ToolError
        } else {
            Outcome::ToolOk
        }),
        Some("posttoolusefailure") => Some(Outcome::ToolError),
        Some(_) => None,
        // Copilot's native payloads carry no event name: tell them apart by field.
        None if value["toolName"].is_string() => {
            if value["toolResult"].is_object() || tool_failed {
                Some(if tool_failed {
                    Outcome::ToolError
                } else {
                    Outcome::ToolOk
                })
            } else {
                None
            }
        }
        None if value["initialPrompt"].is_null() && prompt_text(value).is_some() => {
            Some(Outcome::Prompt)
        }
        None => None,
    }
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value[*key].as_str())
        .map(str::to_string)
}

/// `prompt` is a string (claude, codex, cursor, copilot, pi) or `{text}` (opencode).
fn prompt_text(value: &Value) -> Option<String> {
    let prompt = &value["prompt"];
    prompt
        .as_str()
        .or_else(|| prompt["text"].as_str())
        .map(str::to_string)
}

/// `cwd`, `directory`, or the first of Cursor's `workspace_roots`.
fn cwd(value: &Value) -> Option<String> {
    first_string(value, CWD_KEYS).or_else(|| {
        value["workspace_roots"]
            .get(0)
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn tool_summary(value: &Value, tool: &str) -> String {
    let input = INPUT_KEYS
        .iter()
        .map(|key| &value[*key])
        .find(|input| input.is_object())
        .unwrap_or(&Value::Null);
    let lower = tool.to_ascii_lowercase();
    let found = if COMMAND_TOOLS.contains(&lower.as_str()) {
        input["command"].as_str()
    } else if PATH_TOOLS.contains(&lower.as_str()) {
        PATH_KEYS.iter().find_map(|key| input[*key].as_str())
    } else {
        None
    };
    found.unwrap_or(tool).to_string()
}

fn find_repo(cwd: &str) -> Option<String> {
    Path::new(cwd)
        .ancestors()
        .find(|dir| dir.join(GIT_MARKER).exists())
        .map(|dir| dir.display().to_string())
}
