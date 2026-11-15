use std::path::{Path, PathBuf};

use oncebitten::config::Host;
use oncebitten::triage::host::{control_invocation, invocation, verdict_from, Captured};
use oncebitten::triage::verdict::SCHEMA;

const PROMPT: &str = "classify this";
const VERDICT: &str = r#"{"repeatable":true,"user_correction":false,"root_cause":"wrong tool","proposed_fix_kind":"lint","generality":"general"}"#;

fn workdir() -> PathBuf {
    PathBuf::from("triage-workdir")
}

fn args_of(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

fn position(args: &[&str], flag: &str) -> usize {
    args.iter()
        .position(|arg| *arg == flag)
        .unwrap_or_else(|| panic!("missing {flag} in {args:?}"))
}

#[test]
fn claude_command_uses_the_verified_flags() {
    let inv = invocation(Host::Claude, "haiku", PROMPT, &workdir()).unwrap();
    assert_eq!(inv.program, "claude");
    let args = args_of(&inv.args);
    assert_eq!(args[0], "-p");
    assert_eq!(args[1], PROMPT);
    assert_eq!(args[position(&args, "--output-format") + 1], "json");
    assert_eq!(
        args[position(&args, "--setting-sources") + 1],
        "project,local"
    );
    assert!(args.contains(&"--disable-slash-commands"));
    assert_eq!(args[position(&args, "--tools") + 1], "");
    assert_eq!(args[position(&args, "--json-schema") + 1], SCHEMA);
    assert_eq!(args[position(&args, "--model") + 1], "haiku");
    assert!(args.contains(&"--no-session-persistence"));
    assert!(inv.stdin_null);
    assert!(!inv.streaming);
    assert!(inv.files.is_empty());
    assert!(inv.answer_file.is_none());
}

#[test]
fn default_agent_omits_the_model_flag() {
    let inv = invocation(Host::Claude, "default", PROMPT, &workdir()).unwrap();
    assert!(!inv.args.iter().any(|arg| arg == "--model"));
}

#[test]
fn codex_command_disables_hooks_and_writes_the_answer_file() {
    let dir = workdir();
    let inv = invocation(Host::Codex, "gpt-6.1-terra", PROMPT, &dir).unwrap();
    assert_eq!(inv.program, "codex");
    let args = args_of(&inv.args);
    assert_eq!(args[0], "exec");
    assert!(args.contains(&"--skip-git-repo-check"));
    assert!(args.contains(&"--ephemeral"));
    assert_eq!(args[position(&args, "--disable") + 1], "hooks");
    assert_eq!(args[position(&args, "-s") + 1], "read-only");
    assert_eq!(args[position(&args, "-m") + 1], "gpt-6.1-terra");
    assert!(inv.stdin_null);
    assert!(!inv.streaming);

    let schema = inv
        .files
        .iter()
        .find(|(path, _)| path.starts_with(&dir))
        .expect("schema file written in the workdir");
    assert_eq!(schema.1, SCHEMA);
    let answer = inv.answer_file.as_deref().expect("answer file");
    assert!(answer.starts_with(&dir));
    assert_eq!(args[position(&args, "-o") + 1], answer.to_str().unwrap());
    assert_eq!(
        args[position(&args, "--output-schema") + 1],
        schema.0.to_str().unwrap()
    );
}

#[test]
fn opencode_command_streams_and_names_the_model() {
    let inv = invocation(Host::Opencode, "lemonade/qwen", PROMPT, &workdir()).unwrap();
    assert_eq!(inv.program, "opencode");
    let args = args_of(&inv.args);
    assert_eq!(&args[..4], ["run", "--format", "json", "--agent"]);
    assert_eq!(args[4], "plan");
    assert_eq!(args[position(&args, "-m") + 1], "lemonade/qwen");
    assert!(inv.streaming);
    assert!(inv.stdin_null);
}

#[test]
fn pi_is_unsupported_until_the_spike_verifies_it() {
    let err = invocation(Host::Pi, "default", PROMPT, &workdir()).unwrap_err();
    assert!(
        format!("{err:#}").contains("unsupported host `pi`"),
        "{err:#}"
    );
}

fn captured(stdout: &str, answer: Option<&str>) -> Captured {
    Captured {
        stdout: stdout.to_string(),
        answer: answer.map(str::to_string),
    }
}

#[test]
fn claude_answer_is_the_structured_output_of_the_result_entry() {
    let inv = invocation(Host::Claude, "haiku", PROMPT, &workdir()).unwrap();
    let object: serde_json::Value = serde_json::from_str(VERDICT).unwrap();
    let result = serde_json::json!({"type":"result","subtype":"success","is_error":false,"structured_output":object});
    let as_array = serde_json::json!([{"type":"system"}, result]).to_string();
    let v = verdict_from(&inv, &captured(&as_array, None)).unwrap();
    assert_eq!(v.root_cause, "wrong tool");
    let v = verdict_from(&inv, &captured(&result.to_string(), None)).unwrap();
    assert_eq!(v.root_cause, "wrong tool");
}

#[test]
fn claude_error_or_missing_structured_output_is_not_a_verdict() {
    let inv = invocation(Host::Claude, "haiku", PROMPT, &workdir()).unwrap();
    let failed = r#"{"type":"result","is_error":true,"result":"Credit balance is too low"}"#;
    let no_output = r#"{"type":"result","is_error":false,"result":"text"}"#;
    assert!(verdict_from(&inv, &captured(failed, None)).is_err());
    assert!(verdict_from(&inv, &captured(no_output, None)).is_err());
    assert!(verdict_from(&inv, &captured("not json", None)).is_err());
}

#[test]
fn codex_answer_comes_from_the_answer_file() {
    let inv = invocation(Host::Codex, "default", PROMPT, &workdir()).unwrap();
    assert!(verdict_from(&inv, &captured("", Some(VERDICT))).is_ok());
    assert!(verdict_from(&inv, &captured("", None)).is_err());
    assert!(verdict_from(&inv, &captured("", Some("nope"))).is_err());
}

#[test]
fn opencode_takes_the_first_fenced_verdict_in_the_jsonl_stream() {
    let inv = invocation(Host::Opencode, "default", PROMPT, &workdir()).unwrap();
    let text_event = |text: &str| {
        serde_json::json!({"type":"text","part":{"type":"text","text":text}}).to_string()
    };
    let stream = [
        r#"{"type":"step_start"}"#.to_string(),
        text_event(&format!("```json\n{VERDICT}\n```")),
        text_event("{\"repeatable\":false}"),
    ]
    .join("\n");
    let v = verdict_from(&inv, &captured(&stream, None)).unwrap();
    assert_eq!(v.root_cause, "wrong tool");
}

#[test]
fn opencode_without_a_verdict_is_an_error() {
    let inv = invocation(Host::Opencode, "default", PROMPT, &workdir()).unwrap();
    let stream = r#"{"type":"text","part":{"type":"text","text":"I cannot help"}}"#;
    assert!(verdict_from(&inv, &captured(stream, None)).is_err());
    assert!(verdict_from(&inv, &captured("", None)).is_err());
}

#[test]
fn paths_are_relative_to_the_given_directory() {
    let inv = invocation(Host::Codex, "default", PROMPT, Path::new("w")).unwrap();
    for (path, _) in &inv.files {
        assert!(path.starts_with("w"));
    }
}

#[test]
fn claude_hooks_off_passes_disable_all_hooks_inline() {
    let inv = invocation(Host::Claude, "haiku", PROMPT, &workdir()).unwrap();
    let args = args_of(&inv.args);
    assert_eq!(
        args[position(&args, "--settings") + 1],
        r#"{"disableAllHooks": true}"#
    );
}

#[test]
fn claude_control_has_no_inline_settings() {
    let inv = control_invocation(Host::Claude, "haiku", PROMPT, &workdir()).unwrap();
    assert!(!inv.args.iter().any(|arg| arg == "--settings"));
}
