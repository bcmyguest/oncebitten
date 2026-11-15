mod common;

use std::io::Write;
use std::process::{Command, Output, Stdio};

use common::{both, code, Repo};
use oncebitten::host::{normalize_steer, Host, ALL_HOSTS};
use oncebitten::steer::{render, Decision, SteerEvent};
use serde_json::{json, Value};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn fixture(path: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}/{path}")).unwrap()
}

fn event(payload: &Value) -> SteerEvent {
    normalize_steer(&payload.to_string())
        .unwrap()
        .unwrap()
        .event
}

fn tool(name: &str, command: Option<&str>, paths: &[&str]) -> SteerEvent {
    SteerEvent::Tool {
        tool: name.to_owned(),
        command: command.map(str::to_owned),
        paths: paths.iter().map(|path| (*path).to_owned()).collect(),
    }
}

fn prompt(text: &str) -> SteerEvent {
    SteerEvent::Prompt {
        prompt: text.to_owned(),
    }
}

fn deny() -> Decision {
    Decision {
        deny: vec!["oncebitten no-sudo: ask the user".to_owned()],
        inform: Vec::new(),
    }
}

fn inform() -> Decision {
    Decision {
        deny: Vec::new(),
        inform: vec!["oncebitten gen: regenerate instead".to_owned()],
    }
}

fn rendered(host: Host, event: &SteerEvent, decision: &Decision) -> Option<Value> {
    render(host, event, decision).map(|text| serde_json::from_str(&text).unwrap())
}

#[test]
fn recorded_pre_tool_and_prompt_payloads_normalize() {
    let codex = fixture("codex/PreToolUse.json");
    assert_eq!(
        normalize_steer(&codex).unwrap().unwrap().event,
        tool("Bash", Some("ls; cat nonexistent-file"), &[])
    );
    for (path, text) in [
        (
            "codex/UserPromptSubmit.json",
            "Run the shell command: ls  then run: cat nonexistent-file.  Then say done.",
        ),
        (
            "copilot/userPromptSubmitted.json",
            "Run the shell command ls, then run: cat nonexistent-file. Then say done.",
        ),
        ("pi/before_agent_start.json", "say hi"),
    ] {
        assert_eq!(
            normalize_steer(&fixture(path)).unwrap().unwrap().event,
            prompt(text),
            "{path}"
        );
    }
}

#[test]
fn post_tool_and_session_events_are_not_steered() {
    for path in [
        "codex/PostToolUse.json",
        "copilot/sessionStart.json",
        "cursor/sessionStart.json",
        "pi/tool_result.json",
    ] {
        assert_eq!(normalize_steer(&fixture(path)).unwrap(), None, "{path}");
    }
}

#[test]
fn each_host_pre_tool_shape_normalizes() {
    let cases = [
        // claude, codex and copilot's PascalCase form
        json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "sudo ls"}}),
        // cursor preToolUse
        json!({"hook_event_name": "preToolUse", "tool_name": "Shell", "tool_input": {"command": "sudo ls", "working_directory": "/w"}}),
        // copilot camelCase: no event name, toolArgs as a JSON string
        json!({"sessionId": "s", "toolName": "bash", "toolArgs": "{\"command\":\"sudo ls\"}"}),
        // opencode plugin
        json!({"hook_event_name": "tool.execute.before", "tool": "shell", "input": {"command": "sudo ls"}}),
        // pi extension
        json!({"hook_event_name": "tool_call", "toolName": "bash", "input": {"command": "sudo ls"}}),
    ];
    for payload in cases {
        match event(&payload) {
            SteerEvent::Tool { command, .. } => {
                assert_eq!(command.as_deref(), Some("sudo ls"), "{payload}")
            }
            other => panic!("{payload}: {other:?}"),
        }
    }
}

#[test]
fn edit_paths_become_repo_relative_with_forward_slashes() {
    let repo = Repo::new();
    let root = repo.project();
    let cwd = root.display().to_string();
    let file = root.join("src").join("gen").join("api.rs");
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "cwd": cwd,
        "tool_name": "Edit",
        "tool_input": {"file_path": file.display().to_string()},
    });
    assert_eq!(event(&payload), tool("Edit", None, &["src/gen/api.rs"]));
    let relative = json!({
        "hook_event_name": "tool_call",
        "cwd": cwd,
        "toolName": "write",
        "input": {"path": "docs/x.md"},
    });
    assert_eq!(event(&relative), tool("write", None, &["docs/x.md"]));
}

#[test]
fn patch_tools_yield_every_file_the_patch_names() {
    let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** Add File: src/b.rs\n+z\n*** End Patch\n";
    let codex = json!({"hook_event_name": "PreToolUse", "tool_name": "apply_patch", "tool_input": {"command": patch}});
    match event(&codex) {
        SteerEvent::Tool { paths, .. } => assert_eq!(paths, ["src/a.rs", "src/b.rs"]),
        other => panic!("{other:?}"),
    }
    let opencode = json!({"hook_event_name": "tool.execute.before", "tool": "patch", "input": {"patchText": patch}});
    match event(&opencode) {
        SteerEvent::Tool { paths, .. } => assert_eq!(paths, ["src/a.rs", "src/b.rs"]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn read_tools_carry_no_paths() {
    let payload = json!({"hook_event_name": "PreToolUse", "tool_name": "Read", "tool_input": {"file_path": "src/gen/api.rs"}});
    assert_eq!(event(&payload), tool("Read", None, &[]));
}

#[test]
fn claude_and_codex_deny_and_inform_through_hook_specific_output() {
    let sudo = tool("Bash", Some("sudo ls"), &[]);
    for host in [Host::Claude, Host::Codex] {
        assert_eq!(
            rendered(host, &sudo, &deny()),
            Some(json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "oncebitten no-sudo: ask the user",
            }}))
        );
        assert_eq!(
            rendered(host, &sudo, &inform()),
            Some(json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": "oncebitten gen: regenerate instead",
            }}))
        );
        assert_eq!(
            rendered(host, &prompt("deploy"), &inform()),
            Some(json!({"hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": "oncebitten gen: regenerate instead",
            }}))
        );
    }
}

#[test]
fn cursor_and_copilot_deny_but_cannot_inform() {
    let sudo = tool("Bash", Some("sudo ls"), &[]);
    assert_eq!(
        rendered(Host::Cursor, &sudo, &deny()),
        Some(json!({
            "permission": "deny",
            "user_message": "oncebitten no-sudo: ask the user",
            "agent_message": "oncebitten no-sudo: ask the user",
        }))
    );
    assert_eq!(
        rendered(Host::Copilot, &sudo, &deny()),
        Some(json!({
            "permissionDecision": "deny",
            "permissionDecisionReason": "oncebitten no-sudo: ask the user",
        }))
    );
    for host in [Host::Cursor, Host::Copilot] {
        assert_eq!(rendered(host, &sudo, &inform()), None, "{host:?}");
        assert_eq!(rendered(host, &prompt("x"), &inform()), None, "{host:?}");
    }
}

#[test]
fn plugin_hosts_get_deny_and_context_for_their_scripts() {
    let sudo = tool("bash", Some("sudo ls"), &[]);
    for host in [Host::OpenCode, Host::Pi] {
        assert_eq!(
            rendered(host, &sudo, &deny()),
            Some(json!({"deny": "oncebitten no-sudo: ask the user"}))
        );
        assert_eq!(
            rendered(host, &prompt("x"), &inform()),
            Some(json!({"context": "oncebitten gen: regenerate instead"}))
        );
    }
}

#[test]
fn nothing_matched_renders_nothing() {
    for host in ALL_HOSTS {
        assert_eq!(
            render(host, &prompt("x"), &Decision::default()),
            None,
            "{host:?}"
        );
    }
}

fn steer(repo: &Repo, host: &str, payload: &str, env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oncebitten"));
    command
        .args(["steer", "--host", host])
        .current_dir(repo.project())
        .env("HOME", repo.root.path().join("home"))
        .env("XDG_CONFIG_HOME", repo.root.path().join("xdg-config"))
        .env("XDG_DATA_HOME", repo.root.path().join("data"))
        .env_remove("ONCEBITTEN_TRIAGE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn sudo_payload(repo: &Repo, command: &str) -> String {
    json!({
        "session_id": "s",
        "cwd": repo.project().display().to_string(),
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command},
    })
    .to_string()
}

fn repo_with_no_sudo() -> Repo {
    let repo = Repo::new();
    repo.write(
        ".oncebitten/config.toml",
        "[rules]\nuse_examples = [\"no-sudo\"]\n",
    );
    repo
}

#[test]
fn steer_cli_denies_a_matching_tool_call_for_claude() {
    let repo = repo_with_no_sudo();

    let output = steer(
        &repo,
        "claude",
        &sudo_payload(&repo, "sudo rm -rf /tmp/x"),
        &[],
    );

    assert_eq!(code(&output), 0, "{}", both(&output));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(value["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap()
        .contains("examples/no-sudo"));
}

#[test]
fn steer_cli_prints_nothing_when_no_rule_matches() {
    let repo = repo_with_no_sudo();

    let output = steer(&repo, "claude", &sudo_payload(&repo, "ls -la"), &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    assert!(output.stdout.is_empty(), "{}", both(&output));
}

#[test]
fn steer_cli_ignores_proposed_rules() {
    let repo = Repo::new();
    repo.add_example("no-sudo", "proposed");

    let output = steer(&repo, "claude", &sudo_payload(&repo, "sudo ls"), &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    assert!(output.stdout.is_empty(), "{}", both(&output));
}

#[test]
fn steer_cli_honours_triage() {
    let repo = repo_with_no_sudo();

    let output = steer(
        &repo,
        "claude",
        &sudo_payload(&repo, "sudo ls"),
        &[("ONCEBITTEN_TRIAGE", "1")],
    );

    assert_eq!(code(&output), 0, "{}", both(&output));
    assert!(output.stdout.is_empty(), "{}", both(&output));
}

#[test]
fn steer_cli_fails_open_on_bad_input() {
    let repo = repo_with_no_sudo();
    for (host, payload) in [("claude", "not json"), ("emacs", "{}")] {
        let output = steer(&repo, host, payload, &[]);
        assert_eq!(code(&output), 0, "{}", both(&output));
        assert!(output.stdout.is_empty(), "{}", both(&output));
        assert!(!output.stderr.is_empty(), "{}", both(&output));
    }
}
