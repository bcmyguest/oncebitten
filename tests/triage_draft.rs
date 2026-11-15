use oncebitten::rule::{self, Check, Status};
use oncebitten::triage::draft::Draft;
use oncebitten::triage::verdict::{self, SCHEMA};
use tempfile::TempDir;

const VERDICT: &str = r#""repeatable":true,"user_correction":false,"root_cause":"used datetime.utcnow","proposed_fix_kind":"lint","generality":"general""#;

fn answer(draft: &str) -> String {
    format!("{{{VERDICT},\"draft\":{draft}}}")
}

fn diff_draft() -> serde_json::Value {
    serde_json::json!({
        "id": "docs-with-src",
        "description": "a change under src needs a docs change",
        "mode": "block",
        "engine": "diff",
        "language": null,
        "check": null,
        "command": null,
        "fired_exit_codes": null,
        "fired_output_contains": null,
        "when_changed": ["src/**"],
        "require_changed": ["docs/**"],
        "bad": {"name": "bad.diff", "content": "src/lib.rs\n"},
        "fixed": {"name": "fixed.diff", "content": "src/lib.rs\ndocs/a.md\n"},
        "corpus": [{"name": "only-docs.diff", "content": "docs/a.md\n"}],
        "rationale": "agents forgot docs twice"
    })
}

fn astgrep_draft() -> serde_json::Value {
    serde_json::json!({
        "id": "py-no-utcnow",
        "description": "use datetime.now(timezone.utc)",
        "mode": "block",
        "engine": "ast-grep",
        "language": "python",
        "check": "id: py-no-utcnow\nlanguage: python\nrule:\n  pattern: datetime.utcnow()\nmessage: use datetime.now(timezone.utc)\nseverity: error\n",
        "command": null,
        "fired_exit_codes": null,
        "fired_output_contains": null,
        "when_changed": null,
        "require_changed": null,
        "bad": {"name": "bad.py", "content": "datetime.utcnow()\n"},
        "fixed": {"name": "fixed.py", "content": "datetime.now(timezone.utc)\n"},
        "corpus": [],
        "rationale": "utcnow is deprecated"
    })
}

fn with(mut draft: serde_json::Value, key: &str, value: serde_json::Value) -> serde_json::Value {
    draft[key] = value;
    draft
}

#[test]
fn a_verdict_without_a_draft_still_parses() {
    let answer = verdict::parse_answer(&format!("{{{VERDICT}}}")).unwrap();
    assert!(answer.draft.is_none());
    let answer = verdict::parse_answer(&answer_null()).unwrap();
    assert!(answer.draft.is_none());
}

fn answer_null() -> String {
    format!("{{{VERDICT},\"draft\":null}}")
}

#[test]
fn a_valid_diff_draft_parses_and_writes_a_proposed_rule() {
    let answer = verdict::parse_answer(&answer(&diff_draft().to_string())).unwrap();
    let draft = answer.draft.expect("draft");
    let dir = TempDir::new().unwrap();
    let written = draft.write(dir.path()).unwrap();
    assert_eq!(written, dir.path().join("docs-with-src"));
    let rule = rule::load_rule(&written).unwrap();
    assert_eq!(rule.status, Status::Proposed);
    assert_eq!(
        rule.check,
        Check::Diff {
            when_changed: vec!["src/**".into()],
            require_changed: vec!["docs/**".into()],
        }
    );
    assert!(written.join("fixtures").join("bad.diff").is_file());
    assert!(written.join("corpus").join("only-docs.diff").is_file());
    let text = std::fs::read_to_string(written.join("rule.toml")).unwrap();
    assert!(text.contains("status = \"proposed\""), "{text}");
}

#[test]
fn a_valid_astgrep_draft_writes_its_check_file() {
    let answer = verdict::parse_answer(&answer(&astgrep_draft().to_string())).unwrap();
    let dir = TempDir::new().unwrap();
    let written = answer.draft.unwrap().write(dir.path()).unwrap();
    let rule = rule::load_rule(&written).unwrap();
    assert!(matches!(rule.check, Check::AstGrep { .. }));
    assert_eq!(rule.language.as_deref(), Some("python"));
}

#[test]
fn writing_never_replaces_an_existing_directory() {
    let draft: Draft = serde_json::from_value(diff_draft()).unwrap();
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("docs-with-src")).unwrap();
    assert!(draft.write(dir.path()).is_err());
}

#[test]
fn invalid_drafts_reject_the_whole_answer() {
    let cases = [
        with(diff_draft(), "engine", "regex".into()),
        with(diff_draft(), "id", "Not Kebab".into()),
        with(diff_draft(), "id", "../escape".into()),
        with(diff_draft(), "description", " ".into()),
        with(diff_draft(), "rationale", "".into()),
        with(
            diff_draft(),
            "when_changed",
            serde_json::json!(["src/[oops"]),
        ),
        with(diff_draft(), "when_changed", serde_json::Value::Null),
        with(diff_draft(), "command", serde_json::json!(["true"])),
        with(
            diff_draft(),
            "bad",
            serde_json::json!({"name": "../bad.diff", "content": "x"}),
        ),
        with(
            diff_draft(),
            "bad",
            serde_json::json!({"name": "worse.diff", "content": "x"}),
        ),
        with(
            diff_draft(),
            "fixed",
            serde_json::json!({"name": "fixed.diff", "content": "x".repeat(70_000)}),
        ),
        with(
            diff_draft(),
            "corpus",
            serde_json::json!([{"name": "sub/a.diff", "content": "x"}]),
        ),
        with(diff_draft(), "unknown", 1.into()),
        with(astgrep_draft(), "check", "rule: [unclosed".into()),
        with(astgrep_draft(), "check", serde_json::Value::Null),
        with(astgrep_draft(), "check", "".into()),
        with(
            with(astgrep_draft(), "engine", "native".into()),
            "check",
            serde_json::Value::Null,
        ),
    ];
    for draft in cases {
        let text = answer(&draft.to_string());
        assert!(verdict::parse_answer(&text).is_err(), "accepted: {draft:#}");
        assert!(verdict::parse(&text).is_err(), "parse accepted: {draft:#}");
    }
    let missing_bad = {
        let mut draft = diff_draft();
        draft.as_object_mut().unwrap().remove("bad");
        draft
    };
    assert!(verdict::parse_answer(&answer(&missing_bad.to_string())).is_err());
}

#[test]
fn a_native_draft_needs_a_command_and_fired_exit_codes() {
    let native = serde_json::json!({
        "id": "no-todo",
        "description": "no TODO markers",
        "mode": "steer",
        "engine": "native",
        "language": null,
        "check": null,
        "command": ["grep", "-rq", "TODO", "src"],
        "fired_exit_codes": [1],
        "fired_output_contains": null,
        "when_changed": null,
        "require_changed": null,
        "bad": {"name": "bad.txt", "content": "TODO\n"},
        "fixed": {"name": "fixed.txt", "content": "done\n"},
        "corpus": null,
        "rationale": "agents left TODOs"
    });
    assert!(verdict::parse_answer(&answer(&native.to_string()))
        .unwrap()
        .draft
        .is_some());
    for (key, value) in [
        ("command", serde_json::json!([])),
        ("fired_exit_codes", serde_json::Value::Null),
        ("fired_exit_codes", serde_json::json!([0])),
    ] {
        let bad = with(native.clone(), key, value);
        assert!(
            verdict::parse_answer(&answer(&bad.to_string())).is_err(),
            "{key}"
        );
    }
}

#[test]
fn a_one_off_or_no_fix_verdict_drafts_nothing() {
    for (from, to) in [
        ("\"generality\":\"general\"", "\"generality\":\"one-off\""),
        (
            "\"proposed_fix_kind\":\"lint\"",
            "\"proposed_fix_kind\":\"none\"",
        ),
        ("\"repeatable\":true", "\"repeatable\":false"),
    ] {
        let text = answer(&diff_draft().to_string()).replace(from, to);
        let parsed = verdict::parse_answer(&text).unwrap();
        assert!(parsed.draft.is_none(), "{to}");
    }
}

#[test]
fn crlf_draft_content_is_accepted() {
    let draft = with(
        diff_draft(),
        "bad",
        serde_json::json!({"name": "bad.diff", "content": "src/lib.rs\r\n"}),
    );
    let text = answer(&draft.to_string()).replace('\n', "\r\n");
    assert!(verdict::parse_answer(&text).unwrap().draft.is_some());
}

#[test]
fn schema_describes_an_optional_draft_with_every_draft_key() {
    let schema: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
    let draft = &schema["properties"]["draft"];
    assert!(draft.is_object(), "{schema:#}");
    let props = draft["anyOf"][0]["properties"].as_object().unwrap();
    for key in diff_draft().as_object().unwrap().keys() {
        assert!(props.contains_key(key), "schema lacks draft.{key}");
    }
    assert_eq!(draft["anyOf"][0]["additionalProperties"], false);
    assert_eq!(
        draft["anyOf"][0]["properties"]["engine"]["enum"],
        serde_json::json!(["ast-grep", "diff", "native", "steer"])
    );
}

#[test]
fn the_prompt_asks_for_a_draft() {
    use oncebitten::triage::prompt::{build_prompt, PromptInput};
    let prompt = build_prompt(&PromptInput {
        class: "failure",
        tool: None,
        pattern: "p".into(),
        sessions: 2,
        occurrences: vec![],
    });
    assert!(prompt.contains("draft"), "{prompt}");
    assert!(prompt.contains("null"), "{prompt}");
}

#[test]
fn empty_values_for_unused_keys_count_as_absent() {
    let draft = astgrep_draft();
    let draft = with(draft, "command", serde_json::json!([]));
    let draft = with(draft, "fired_exit_codes", serde_json::json!([]));
    let draft = with(draft, "fired_output_contains", serde_json::json!(""));
    let draft = with(draft, "when_changed", serde_json::json!([]));
    let draft = with(draft, "require_changed", serde_json::json!([]));
    let answer = verdict::parse_answer(&answer(&draft.to_string())).unwrap();
    let draft = answer.draft.expect("draft kept");
    assert!(draft.command.is_none());
    assert!(draft.when_changed.is_none());
    assert!(draft.fired_output_contains.is_none());
}

#[test]
fn names_a_windows_host_cannot_write_are_rejected() {
    for name in [
        "a?.rs",
        "x*.rs",
        "nul.json",
        "con.rs",
        "CON.txt",
        "com1.diff",
        "lpt9",
        "a.diff.",
        "a.diff ",
        "a<b.diff",
        "a>b.diff",
        "a|b.diff",
        "a\"b.diff",
        "a\u{1}b.diff",
    ] {
        let draft = with(
            diff_draft(),
            "corpus",
            serde_json::json!([{"name": name, "content": "x"}]),
        );
        let text = answer(&draft.to_string());
        assert!(verdict::parse_answer(&text).is_err(), "accepted {name:?}");
    }
    let ok = with(
        diff_draft(),
        "corpus",
        serde_json::json!([{"name": "console.diff", "content": "x"}]),
    );
    assert!(verdict::parse_answer(&answer(&ok.to_string())).is_ok());
}

#[cfg(target_os = "linux")]
#[test]
fn a_write_failing_midway_leaves_no_rule_directory() {
    let long = format!("{}.diff", "a".repeat(200));
    let draft = with(
        diff_draft(),
        "corpus",
        serde_json::json!([{"name": long, "content": "x"}]),
    );
    let answer = verdict::parse_answer(&answer(&draft.to_string())).unwrap();
    let root = TempDir::new().unwrap();
    let mut parent = root.path().to_path_buf();
    while parent.as_os_str().len() < 4020 {
        parent.push("d".repeat(200));
        std::fs::create_dir(&parent).unwrap();
    }
    assert!(answer.draft.unwrap().write(&parent).is_err());
    assert!(!parent.join("docs-with-src").exists());
}

fn steer_event(tool: &str, command: &str) -> String {
    serde_json::json!({"event": "tool", "tool": tool, "command": command}).to_string()
}

fn steer_draft(mode: &str, engine: &str, bad: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "no-force-push",
        "description": "no force pushes",
        "mode": mode,
        "engine": engine,
        "bad": {"name": "bad.json", "content": bad},
        "fixed": {"name": "fixed.json", "content": steer_event("Bash", "git push")},
        "rationale": "force pushes rewrite shared history"
    })
}

#[test]
fn a_steer_engine_draft_needs_mode_steer() {
    let draft = steer_draft("block", "steer", &steer_event("Bash", "git push --force"));
    let parsed: Draft = serde_json::from_value(draft).unwrap();
    assert!(parsed.validate().is_err());
}

#[test]
fn a_steer_draft_writes_an_inform_rule_with_the_recorded_command_as_bad() {
    let draft = steer_draft(
        "steer",
        "steer",
        &steer_event("Bash", "git push --force origin main"),
    );
    let parsed: Draft = serde_json::from_value(draft).unwrap();
    parsed.validate().unwrap();
    let parent = TempDir::new().unwrap();
    let dir = parsed.write(parent.path()).unwrap();
    let text = std::fs::read_to_string(dir.join("rule.toml")).unwrap();
    assert!(text.contains("engine = \"steer\""), "{text}");
    assert!(text.contains("[steer]"), "{text}");
    assert!(text.contains("action = \"inform\""), "{text}");
    let rule = rule::load_rule(&dir).unwrap();
    let Check::Steer(steer) = &rule.check else {
        panic!("expected a steer rule, got {:?}", rule.check);
    };
    assert_eq!(steer.action, rule::Action::Inform);
    assert_eq!(
        oncebitten::steer::replay(&dir, steer),
        oncebitten::replay::Verdict::Proven
    );
}

#[test]
fn a_steer_fixture_must_be_a_recorded_tool_event() {
    let draft = steer_draft("steer", "steer", "git push --force");
    let parsed: Draft = serde_json::from_value(draft).unwrap();
    assert!(parsed.validate().is_err());
}
