use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::config::Host;
use crate::triage::verdict::{self, Verdict, SCHEMA};

const SCHEMA_FILE: &str = "schema.json";
const ANSWER_FILE: &str = "answer.json";
/// The agent name that means "the host's configured default model".
pub const DEFAULT_AGENT: &str = "default";

/// One headless triage call, fully specified. Nothing here runs a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    /// Files to create in the workdir before the call: (path, contents).
    pub files: Vec<(PathBuf, String)>,
    /// The file the host writes its answer to, when it writes one.
    pub answer_file: Option<PathBuf>,
    /// Stdin is closed: some hosts block on an open stdin.
    pub stdin_null: bool,
    /// The host may print its answer and keep running; the caller must parse
    /// the output and kill the process.
    pub streaming: bool,
}

/// What the runner captured from one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub stdout: String,
    /// Contents of `Invocation::answer_file`, if the host wrote it.
    pub answer: Option<String>,
}

/// Builds the command for `host` at `agent` (a model name, or `default`).
/// Hosts without a verified headless JSON call are rejected here.
pub fn invocation(host: Host, agent: &str, prompt: &str, workdir: &Path) -> Result<Invocation> {
    let model = (agent != DEFAULT_AGENT).then_some(agent);
    match host {
        Host::Claude => Ok(claude(model, prompt)),
        Host::Codex => Ok(codex(model, prompt, workdir)),
        Host::Opencode => Ok(opencode(model, prompt)),
        Host::Pi => bail!(
            "unsupported host `{}`: no verified headless JSON call (see docs/research/spike-triage.md)",
            host.as_str()
        ),
    }
}

fn claude(model: Option<&str>, prompt: &str) -> Invocation {
    let mut args = strings(&[
        "-p",
        prompt,
        "--output-format",
        "json",
        "--setting-sources",
        "project,local",
        "--disable-slash-commands",
        "--tools",
        "",
        "--json-schema",
        SCHEMA,
        "--no-session-persistence",
    ]);
    push_model(&mut args, "--model", model);
    Invocation {
        program: "claude".into(),
        args,
        files: Vec::new(),
        answer_file: None,
        stdin_null: true,
        streaming: false,
    }
}

fn codex(model: Option<&str>, prompt: &str, workdir: &Path) -> Invocation {
    let schema_path = workdir.join(SCHEMA_FILE);
    let answer_path = workdir.join(ANSWER_FILE);
    let mut args = strings(&[
        "exec",
        "--skip-git-repo-check",
        "--json",
        "--ephemeral",
        "--disable",
        "hooks",
        "-s",
        "read-only",
    ]);
    push_model(&mut args, "-m", model);
    args.push("--output-schema".into());
    args.push(path_arg(&schema_path));
    args.push("-o".into());
    args.push(path_arg(&answer_path));
    args.push(prompt.to_string());
    Invocation {
        program: "codex".into(),
        args,
        files: vec![(schema_path, SCHEMA.to_string())],
        answer_file: Some(answer_path),
        stdin_null: true,
        streaming: false,
    }
}

fn opencode(model: Option<&str>, prompt: &str) -> Invocation {
    let mut args = strings(&["run", "--format", "json", "--agent", "plan"]);
    push_model(&mut args, "-m", model);
    args.push(prompt.to_string());
    Invocation {
        program: "opencode".into(),
        args,
        files: Vec::new(),
        answer_file: None,
        stdin_null: true,
        streaming: true,
    }
}

/// Reads the verdict from what the call printed or wrote. Anything else is an
/// error, which the caller reports as "could not run".
pub fn verdict_from(inv: &Invocation, captured: &Captured) -> Result<Verdict> {
    match inv.program.as_str() {
        "claude" => claude_verdict(&captured.stdout),
        "codex" => {
            let answer = captured
                .answer
                .as_deref()
                .context("codex wrote no answer file")?;
            verdict::parse(answer)
        }
        "opencode" => opencode_verdict(&captured.stdout),
        other => bail!("no answer reader for `{other}`"),
    }
}

fn claude_verdict(stdout: &str) -> Result<Verdict> {
    let value: Value = serde_json::from_str(stdout).context("claude printed no JSON")?;
    let result = match &value {
        Value::Array(entries) => entries
            .iter()
            .rev()
            .find(|entry| entry["type"] == "result")
            .context("claude printed no result entry")?,
        _ => &value,
    };
    if result["is_error"] == true {
        bail!("claude reported an error");
    }
    let structured = &result["structured_output"];
    if structured.is_null() {
        bail!("claude result has no structured_output");
    }
    verdict::parse(&structured.to_string())
}

/// Each text part is tried as a verdict; the first that parses wins.
fn opencode_verdict(stdout: &str) -> Result<Verdict> {
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event["type"] != "text" {
            continue;
        }
        let text = event["part"]["text"]
            .as_str()
            .or_else(|| event["text"].as_str());
        if let Some(text) = text {
            if let Ok(verdict) = verdict::parse(&strip_fence(text)) {
                return Ok(verdict);
            }
        }
    }
    bail!("opencode printed no verdict")
}

fn strip_fence(text: &str) -> String {
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    inner
        .strip_suffix("```")
        .unwrap_or(inner)
        .trim()
        .to_string()
}

fn push_model(args: &mut Vec<String>, flag: &str, model: Option<&str>) {
    if let Some(model) = model {
        args.push(flag.to_string());
        args.push(model.to_string());
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
