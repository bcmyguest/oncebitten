use std::fs;

use oncebitten::host::{normalize, Host};
use oncebitten::signal::{KIND_PROMPT, KIND_TOOL_ERROR, KIND_TOOL_OK};
use tempfile::TempDir;

const TS: i64 = 1_000;

fn claude_prompt(cwd: &str) -> String {
    // JSON-escape cwd: Windows paths contain backslashes.
    let cwd = serde_json::to_string(cwd).unwrap();
    format!(
        r#"{{"session_id":"sess-1","transcript_path":"/x.jsonl","cwd":{cwd},"hook_event_name":"UserPromptSubmit","prompt":"No, use tabs"}}"#
    )
}

fn claude_post(event: &str, tool: &str, input: &str) -> String {
    format!(
        r#"{{"session_id":"sess-1","cwd":"/nowhere","hook_event_name":"{event}","tool_name":"{tool}","tool_input":{input},"tool_use_id":"toolu_1"}}"#
    )
}

#[test]
fn host_names_parse_and_unknown_names_fail() {
    assert_eq!(Host::parse("claude").unwrap(), Host::Claude);
    assert_eq!(Host::parse("opencode").unwrap(), Host::OpenCode);
    assert!(Host::parse("emacs").is_err());
}

#[test]
fn claude_prompt_becomes_a_prompt_event() {
    let event = normalize(Host::Claude, &claude_prompt("/nowhere"), TS)
        .unwrap()
        .unwrap();
    assert_eq!(event.host, "claude");
    assert_eq!(event.session_id, "sess-1");
    assert_eq!(event.ts, TS);
    assert_eq!(event.kind, KIND_PROMPT);
    assert_eq!(event.tool, None);
    assert_eq!(event.summary, "No, use tabs");
}

#[test]
fn claude_post_tool_use_records_the_bash_command() {
    let payload = claude_post(
        "PostToolUse",
        "Bash",
        r#"{"command":"cargo test --lib","description":"run"}"#,
    );
    let event = normalize(Host::Claude, &payload, TS).unwrap().unwrap();
    assert_eq!(event.kind, KIND_TOOL_OK);
    assert_eq!(event.tool.as_deref(), Some("Bash"));
    assert_eq!(event.summary, "cargo test --lib");
}

#[test]
fn claude_post_tool_use_failure_is_a_tool_error() {
    let payload = claude_post(
        "PostToolUseFailure",
        "Bash",
        r#"{"command":"sh -c 'exit 3'"}"#,
    );
    let event = normalize(Host::Claude, &payload, TS).unwrap().unwrap();
    assert_eq!(event.kind, KIND_TOOL_ERROR);
    assert_eq!(event.summary, "sh -c 'exit 3'");
}

#[test]
fn claude_file_edit_records_the_file_path() {
    let payload = claude_post(
        "PostToolUse",
        "Edit",
        r#"{"file_path":"src/a.rs","old_string":"a","new_string":"b"}"#,
    );
    let event = normalize(Host::Claude, &payload, TS).unwrap().unwrap();
    assert_eq!(event.summary, "src/a.rs");
}

#[test]
fn other_claude_events_are_ignored() {
    let payload = r#"{"session_id":"s","hook_event_name":"SessionStart"}"#;
    assert!(normalize(Host::Claude, payload, TS).unwrap().is_none());
}

#[test]
fn repo_is_the_nearest_ancestor_with_a_git_marker() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    let nested = repo.join("src").join("deep");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(&nested).unwrap();
    let payload = claude_prompt(nested.to_str().unwrap());
    let event = normalize(Host::Claude, &payload, TS).unwrap().unwrap();
    assert_eq!(event.repo.as_deref(), repo.to_str());
}

#[test]
fn cwd_outside_any_repo_has_no_repo() {
    let payload = claude_prompt("/nowhere/project");
    let event = normalize(Host::Claude, &payload, TS).unwrap().unwrap();
    assert_eq!(event.repo, None);
}

#[test]
fn malformed_payload_is_an_error() {
    assert!(normalize(Host::Claude, "not json", TS).is_err());
}

#[test]
fn missing_session_id_is_an_error() {
    let payload = r#"{"hook_event_name":"UserPromptSubmit","prompt":"x"}"#;
    assert!(normalize(Host::Claude, payload, TS).is_err());
}
