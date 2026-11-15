use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::signal::{KIND_PROMPT, KIND_TOOL_ERROR, KIND_TOOL_OK};
use crate::store::NewEvent;

const GIT_MARKER: &str = ".git";
const PATH_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "Read"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Claude,
    Codex,
    OpenCode,
    Cursor,
    Pi,
}

impl Host {
    pub fn parse(name: &str) -> Result<Host> {
        match name {
            "claude" => Ok(Host::Claude),
            "codex" => Ok(Host::Codex),
            "opencode" => Ok(Host::OpenCode),
            "cursor" => Ok(Host::Cursor),
            "pi" => Ok(Host::Pi),
            other => {
                bail!("unknown host `{other}`; expected claude, codex, opencode, cursor or pi")
            }
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Host::Claude => "claude",
            Host::Codex => "codex",
            Host::OpenCode => "opencode",
            Host::Cursor => "cursor",
            Host::Pi => "pi",
        }
    }
}

pub fn normalize(host: Host, payload: &str, ts: i64) -> Result<Option<NewEvent>> {
    match host {
        Host::Claude => claude(payload, ts),
        Host::Codex | Host::OpenCode | Host::Cursor | Host::Pi => {
            bail!(
                "unsupported host `{}`: its hook payload format is not verified",
                host.name()
            )
        }
    }
}

fn claude(payload: &str, ts: i64) -> Result<Option<NewEvent>> {
    let value: Value = serde_json::from_str(payload).context("hook payload is not JSON")?;
    let session_id = string_field(&value, "session_id")?;
    let repo = value["cwd"].as_str().and_then(find_repo);
    let (kind, tool, summary) = match value["hook_event_name"].as_str() {
        Some("UserPromptSubmit") => (KIND_PROMPT, None, string_field(&value, "prompt")?),
        Some(event @ ("PostToolUse" | "PostToolUseFailure")) => {
            let kind = if event == "PostToolUse" {
                KIND_TOOL_OK
            } else {
                KIND_TOOL_ERROR
            };
            let tool = string_field(&value, "tool_name")?;
            let summary = claude_tool_summary(&value, &tool);
            (kind, Some(tool), summary)
        }
        _ => return Ok(None),
    };
    Ok(Some(NewEvent {
        ts,
        host: Host::Claude.name().to_string(),
        session_id,
        repo,
        kind: kind.to_string(),
        tool,
        summary,
    }))
}

fn claude_tool_summary(value: &Value, tool: &str) -> String {
    let input = &value["tool_input"];
    let field = if tool == "Bash" {
        "command"
    } else if PATH_TOOLS.contains(&tool) {
        "file_path"
    } else {
        return tool.to_string();
    };
    input[field].as_str().unwrap_or(tool).to_string()
}

fn string_field(value: &Value, name: &str) -> Result<String> {
    value[name]
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("hook payload has no `{name}` string"))
}

fn find_repo(cwd: &str) -> Option<String> {
    Path::new(cwd)
        .ancestors()
        .find(|dir| dir.join(GIT_MARKER).exists())
        .map(|dir| dir.display().to_string())
}
