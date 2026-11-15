use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::signal::{
    KIND_PROMPT, KIND_TOOL_ERROR, KIND_TOOL_OK, REPEAT_FAILURE, REVERT, TEST_FAIL_THEN_PASS,
    TOOL_FAILURE, USER_CORRECTION,
};
use crate::steer::SteerEvent;
use crate::store::NewEvent;

const GIT_MARKER: &str = ".git";
/// Lowercase names of the shell tools: `Bash` (Claude Code, Codex), `Shell`
/// (Cursor), `shell` (opencode), `bash` (pi, Copilot).
pub const COMMAND_TOOLS: &[&str] = &["bash", "shell"];
/// Lowercase names of the tools that change a file: `Edit`, `Write`, `MultiEdit`
/// (Claude Code, Cursor) and `edit`, `write` (pi, Copilot, opencode).
pub const EDIT_TOOLS: &[&str] = &["edit", "write", "multiedit"];
const PATH_TOOLS: &[&str] = &["edit", "write", "multiedit", "read"];
const SESSION_KEYS: &[&str] = &["session_id", "sessionId", "sessionID", "conversation_id"];
const EVENT_KEYS: &[&str] = &["hook_event_name", "hookEventName"];
const CWD_KEYS: &[&str] = &["cwd", "directory"];
const TOOL_KEYS: &[&str] = &["tool_name", "toolName", "tool"];
const INPUT_KEYS: &[&str] = &["tool_input", "toolInput", "toolArgs", "input"];
const PATH_KEYS: &[&str] = &["file_path", "filePath", "path"];
/// Keys that name the file an edit or write tool touches. Codex `apply_patch`
/// and opencode `patch` carry patch text instead; see [`PATCH_KEYS`].
const EDIT_PATH_KEYS: &[&str] = &["file_path", "filePath", "path", "notebook_path"];
const PATCH_KEYS: &[&str] = &["patchText", "patch", "input"];
const PATCH_TOOLS: &[&str] = &["apply_patch", "patch"];
const PATCH_HEADERS: &[&str] = &[
    "*** Add File: ",
    "*** Update File: ",
    "*** Delete File: ",
    "*** Move to: ",
];
/// Tools that only read: their paths never trigger a steer path rule.
const READ_TOOLS: &[&str] = &["read", "view", "glob", "grep", "ls", "list"];
const PRE_TOOL_EVENTS: &[&str] = &["pretooluse", "tool_call", "tool.execute.before"];
const PROMPT_EVENTS: &[&str] = &[
    "userpromptsubmit",
    "userpromptsubmitted",
    "beforesubmitprompt",
    "before_agent_start",
    "prompt",
];

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

impl Capabilities {
    /// The signals a host can produce, derived from what it reports: prompts
    /// feed `user-correction`; tool outcomes feed every tool-based signal.
    pub fn signals(self) -> Vec<&'static str> {
        let mut signals = Vec::new();
        if self.prompt {
            signals.push(USER_CORRECTION);
        }
        if self.tool_outcome {
            signals.extend([TOOL_FAILURE, REPEAT_FAILURE, TEST_FAIL_THEN_PASS, REVERT]);
        }
        signals
    }
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
        Some(name) if PROMPT_EVENTS.contains(&name) => Some(Outcome::Prompt),
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

/// A hook payload as steer rules see it, plus the directory the agent works in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteerInput {
    pub event: SteerEvent,
    pub cwd: Option<String>,
}

/// Maps a pre-tool or prompt hook payload onto a steer event. Every other
/// event (post-tool, session start, stop) maps to `None`.
pub fn normalize_steer(payload: &str) -> Result<Option<SteerInput>> {
    let value: Value = serde_json::from_str(payload).context("hook payload is not JSON")?;
    let event_name = first_string(&value, EVENT_KEYS).map(|name| name.to_ascii_lowercase());
    let is_tool = match event_name.as_deref() {
        Some(name) => PRE_TOOL_EVENTS.contains(&name),
        // Copilot's native payloads carry no event name: a tool name without a
        // result is its preToolUse.
        None => value["toolName"].is_string() && value["toolResult"].is_null(),
    };
    let is_prompt = match event_name.as_deref() {
        Some(name) => PROMPT_EVENTS.contains(&name),
        None => !is_tool && value["initialPrompt"].is_null() && prompt_text(&value).is_some(),
    };
    let cwd = cwd(&value);
    let event = if is_tool {
        let tool = first_string(&value, TOOL_KEYS).context("hook payload has no tool name")?;
        let input = tool_input(&value);
        let command = input["command"].as_str().map(str::to_string);
        let paths = edited_paths(&tool, &input)
            .into_iter()
            .map(|path| repo_relative(&path, cwd.as_deref()))
            .collect();
        SteerEvent::Tool {
            tool,
            command,
            paths,
        }
    } else if is_prompt {
        let prompt = prompt_text(&value).context("hook payload has no prompt text")?;
        SteerEvent::Prompt { prompt }
    } else {
        return Ok(None);
    };
    Ok(Some(SteerInput { event, cwd }))
}

/// The tool's input object. Copilot's camelCase payload sends it as a JSON
/// string in `toolArgs`.
fn tool_input(value: &Value) -> Value {
    for key in INPUT_KEYS {
        match &value[*key] {
            Value::Object(_) => return value[*key].clone(),
            Value::String(text) => {
                if let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(text) {
                    return parsed;
                }
            }
            _ => {}
        }
    }
    Value::Null
}

/// The files an edit or write tool touches, as the payload names them.
fn edited_paths(tool: &str, input: &Value) -> Vec<String> {
    let lower = tool.to_ascii_lowercase();
    if READ_TOOLS.contains(&lower.as_str()) {
        return Vec::new();
    }
    if PATCH_TOOLS.contains(&lower.as_str()) {
        let text = PATCH_KEYS
            .iter()
            .chain(["command"].iter())
            .find_map(|key| input[*key].as_str())
            .unwrap_or_default();
        return text
            .lines()
            .filter_map(|line| {
                PATCH_HEADERS
                    .iter()
                    .find_map(|header| line.strip_prefix(header))
            })
            .map(|path| path.trim().to_string())
            .collect();
    }
    EDIT_PATH_KEYS
        .iter()
        .find_map(|key| input[*key].as_str())
        .map(|path| vec![path.to_string()])
        .unwrap_or_default()
}

/// `path` relative to the repository holding `cwd`, `/`-separated. A path
/// outside the repository stays as given, with `/` separators.
fn repo_relative(path: &str, cwd: Option<&str>) -> String {
    let given = Path::new(path);
    let absolute = match cwd {
        Some(cwd) if given.is_relative() => Path::new(cwd).join(given),
        _ => given.to_path_buf(),
    };
    let relative = cwd
        .and_then(find_repo)
        .and_then(|repo| absolute.strip_prefix(&repo).ok().map(Path::to_path_buf));
    match relative {
        Some(relative) => relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        None => path.replace('\\', "/"),
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

pub fn is_command_tool(tool: &str) -> bool {
    COMMAND_TOOLS.contains(&tool.to_ascii_lowercase().as_str())
}

pub fn is_edit_tool(tool: &str) -> bool {
    EDIT_TOOLS.contains(&tool.to_ascii_lowercase().as_str())
}

fn tool_summary(value: &Value, tool: &str) -> String {
    let input = &tool_input(value);
    let lower = tool.to_ascii_lowercase();
    let found = if is_command_tool(tool) {
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
