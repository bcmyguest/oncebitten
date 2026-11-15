use std::fs;

use oncebitten::host::{normalize, Host, ALL_HOSTS};
use oncebitten::signal::{KIND_PROMPT, KIND_TOOL_ERROR, KIND_TOOL_OK};

const TS: i64 = 7;

fn fixture(host: &str, name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/{host}/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("{path}: {err}"))
}

#[test]
fn every_host_rejects_an_empty_payload_for_its_missing_session_id() {
    for host in ALL_HOSTS {
        let err = normalize(host, "{}", TS).unwrap_err().to_string();
        assert!(err.contains("no session id"), "{host:?}: {err}");
    }
}

#[test]
fn non_json_fails_for_every_host() {
    for host in ALL_HOSTS {
        assert!(normalize(host, "nope", TS).is_err());
    }
}

#[test]
fn host_names_round_trip() {
    for host in ALL_HOSTS {
        assert_eq!(Host::parse(host.name()).unwrap(), host);
    }
    assert!(Host::parse("emacs").is_err());
}

#[test]
fn codex_real_prompt_is_a_prompt() {
    let event = normalize(Host::Codex, &fixture("codex", "UserPromptSubmit"), TS)
        .unwrap()
        .unwrap();
    assert_eq!(event.host, "codex");
    assert_eq!(event.session_id, "01a1dcc7-2d7a-7f80-a567-91fb61812977");
    assert_eq!(event.kind, KIND_PROMPT);
    assert_eq!(event.tool, None);
    assert!(event.summary.starts_with("Run the shell command"));
    assert_eq!(event.repo, None, "/work/project does not exist here");
}

#[test]
fn codex_tool_events_are_skipped_because_the_payload_has_no_outcome() {
    for name in ["PreToolUse", "PostToolUse"] {
        assert!(normalize(Host::Codex, &fixture("codex", name), TS)
            .unwrap()
            .is_none());
    }
}

#[test]
fn cursor_session_start_is_ignored() {
    let payload = fixture("cursor", "sessionStart");
    assert!(normalize(Host::Cursor, &payload, TS).unwrap().is_none());
}

#[test]
fn cursor_prompt_uses_conversation_id_and_workspace_root() {
    // Field names from https://cursor.com/docs/hooks (not captured live).
    let dir = tempfile::TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    let payload = serde_json::json!({
        "conversation_id": "c-1", "generation_id": "g-1",
        "hook_event_name": "beforeSubmitPrompt", "prompt": "use tabs",
        "attachments": [], "workspace_roots": [dir.path()],
    })
    .to_string();
    let event = normalize(Host::Cursor, &payload, TS).unwrap().unwrap();
    assert_eq!(event.session_id, "c-1");
    assert_eq!(event.kind, KIND_PROMPT);
    assert_eq!(event.summary, "use tabs");
    assert_eq!(event.repo.as_deref(), dir.path().to_str());
}

#[test]
fn cursor_tool_events_follow_the_documented_fields() {
    let ok = r#"{"conversation_id":"c","hook_event_name":"postToolUse","tool_name":"Shell","tool_input":{"command":"ls"},"tool_output":"{}","cwd":"/nowhere","duration":5}"#;
    let event = normalize(Host::Cursor, ok, TS).unwrap().unwrap();
    assert_eq!(
        (
            event.kind.as_str(),
            event.tool.as_deref(),
            event.summary.as_str()
        ),
        (KIND_TOOL_OK, Some("Shell"), "ls")
    );
    let failed = r#"{"conversation_id":"c","hook_event_name":"postToolUseFailure","tool_name":"Shell","tool_input":{"command":"false"},"error_message":"x","failure_type":"error"}"#;
    let event = normalize(Host::Cursor, failed, TS).unwrap().unwrap();
    assert_eq!(event.kind, KIND_TOOL_ERROR);
    assert_eq!(event.summary, "false");
}

#[test]
fn copilot_real_prompt_has_no_event_name_and_is_still_a_prompt() {
    let event = normalize(
        Host::Copilot,
        &fixture("copilot", "userPromptSubmitted"),
        TS,
    )
    .unwrap()
    .unwrap();
    assert_eq!(event.session_id, "51d94bf4-27b4-4dc9-8fc6-bc0c691f4d01");
    assert_eq!(event.kind, KIND_PROMPT);
}

#[test]
fn copilot_session_start_is_ignored() {
    let payload = fixture("copilot", "sessionStart");
    assert!(normalize(Host::Copilot, &payload, TS).unwrap().is_none());
}

#[test]
fn copilot_tool_events_follow_the_documented_fields() {
    // Field names from the GitHub Copilot hooks reference (not captured live).
    let ok = r#"{"sessionId":"s","timestamp":1,"cwd":"/nowhere","toolName":"bash","toolArgs":{"command":"ls"},"toolResult":{"resultType":"success","textResultForLlm":"a"}}"#;
    let event = normalize(Host::Copilot, ok, TS).unwrap().unwrap();
    assert_eq!(event.kind, KIND_TOOL_OK);
    assert_eq!(event.summary, "ls");
    let failed = r#"{"sessionId":"s","timestamp":1,"cwd":"/nowhere","toolName":"bash","toolArgs":{"command":"false"},"error":"boom"}"#;
    let event = normalize(Host::Copilot, failed, TS).unwrap().unwrap();
    assert_eq!(event.kind, KIND_TOOL_ERROR);
    let vscode = r#"{"session_id":"s","hook_event_name":"PostToolUseFailure","tool_name":"bash","tool_input":{"command":"x"}}"#;
    assert_eq!(
        normalize(Host::Copilot, vscode, TS).unwrap().unwrap().kind,
        KIND_TOOL_ERROR
    );
}

#[test]
fn opencode_real_prompt_payload_keeps_its_text_object() {
    let mut value: serde_json::Value =
        serde_json::from_str(&fixture("opencode", "prompt")).unwrap();
    // The forwarding plugin adds the event name and the project directory.
    value["hook_event_name"] = "prompt".into();
    value["directory"] = "/nowhere".into();
    let event = normalize(Host::OpenCode, &value.to_string(), TS)
        .unwrap()
        .unwrap();
    assert_eq!(event.session_id, "ses_e2329b551ffewSSutV06Q1hu35");
    assert_eq!(event.kind, KIND_PROMPT);
    assert!(event.summary.contains("cat nonexistent-file"));
}

#[test]
fn opencode_tool_events_are_skipped_because_the_payload_is_unverified() {
    let done = r#"{"hook_event_name":"tool.execute.after","sessionID":"s","tool":"bash","status":"completed"}"#;
    assert!(normalize(Host::OpenCode, done, TS).unwrap().is_none());
}

#[test]
fn pi_events_follow_the_documented_fields() {
    let prompt = r#"{"hook_event_name":"before_agent_start","session_id":"p","cwd":"/nowhere","prompt":"hello"}"#;
    assert_eq!(
        normalize(Host::Pi, prompt, TS).unwrap().unwrap().kind,
        KIND_PROMPT
    );
    let tool = r#"{"hook_event_name":"tool_result","session_id":"p","cwd":"/nowhere","toolName":"read","input":{"path":"a.rs"},"isError":false}"#;
    let event = normalize(Host::Pi, tool, TS).unwrap().unwrap();
    assert_eq!(
        (event.kind.as_str(), event.summary.as_str()),
        (KIND_TOOL_OK, "a.rs")
    );
    let bad = r#"{"hook_event_name":"tool_result","session_id":"p","toolName":"bash","input":{"command":"x"},"isError":true}"#;
    assert_eq!(
        normalize(Host::Pi, bad, TS).unwrap().unwrap().kind,
        KIND_TOOL_ERROR
    );
}

#[test]
fn session_id_aliases_all_resolve() {
    for key in ["session_id", "sessionId", "sessionID", "conversation_id"] {
        let payload =
            format!(r#"{{"{key}":"x","hook_event_name":"UserPromptSubmit","prompt":"p"}}"#);
        let event = normalize(Host::Claude, &payload, TS).unwrap().unwrap();
        assert_eq!(event.session_id, "x", "{key}");
    }
}

#[test]
fn event_name_and_prompt_object_aliases_resolve() {
    let named = r#"{"sessionId":"s","hookEventName":"UserPromptSubmit","prompt":{"text":"t"}}"#;
    assert_eq!(
        normalize(Host::Claude, named, TS).unwrap().unwrap().summary,
        "t"
    );
}

#[test]
fn fixtures_hold_no_personal_data() {
    for host in ["codex", "cursor", "copilot", "opencode", "pi"] {
        let dir = format!("{}/tests/fixtures/{host}", env!("CARGO_MANIFEST_DIR"));
        for entry in fs::read_dir(dir).unwrap() {
            let text = fs::read_to_string(entry.unwrap().path()).unwrap();
            for needle in ["gmail", "/home/", "/tmp/", "obcap"] {
                assert!(!text.contains(needle), "{host}: {needle}");
            }
        }
    }
}

#[test]
fn pi_fixtures_from_a_real_run_normalize() {
    let prompt = normalize(Host::Pi, &fixture("pi", "before_agent_start"), TS)
        .unwrap()
        .unwrap();
    assert_eq!(prompt.kind, KIND_PROMPT);
    assert_eq!(prompt.summary, "say hi");
    let tool = normalize(Host::Pi, &fixture("pi", "tool_result"), TS)
        .unwrap()
        .unwrap();
    assert_eq!(tool.kind, KIND_TOOL_OK);
    assert_eq!(tool.tool.as_deref(), Some("bash"));
}
