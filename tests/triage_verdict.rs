use oncebitten::triage::prompt::{bound, build_prompt, Occurrence, PromptInput};
use oncebitten::triage::verdict::{self, FixKind, Generality, Verdict, SCHEMA};

const VALID: &str = r#"{"repeatable":true,"user_correction":true,"root_cause":"ran python3 -c instead of the notebook","proposed_fix_kind":"agent-workflow","generality":"project"}"#;

#[test]
fn parses_a_complete_verdict() {
    let verdict = verdict::parse(VALID).unwrap();
    assert_eq!(
        verdict,
        Verdict {
            repeatable: true,
            user_correction: true,
            root_cause: "ran python3 -c instead of the notebook".into(),
            proposed_fix_kind: FixKind::AgentWorkflow,
            generality: Generality::Project,
            mistake_class: None,
        }
    );
}

#[test]
fn every_fix_kind_and_generality_uses_its_schema_spelling() {
    for (text, want) in [
        ("harness", FixKind::Harness),
        ("agent-workflow", FixKind::AgentWorkflow),
        ("lint", FixKind::Lint),
        ("test", FixKind::Test),
        ("test-practice", FixKind::TestPractice),
        ("invariant", FixKind::Invariant),
        ("none", FixKind::NoFix),
    ] {
        let json = VALID.replace("agent-workflow", text);
        assert_eq!(verdict::parse(&json).unwrap().proposed_fix_kind, want);
    }
    for (text, want) in [
        ("general", Generality::General),
        ("project", Generality::Project),
        ("one-off", Generality::OneOff),
    ] {
        let json = VALID.replace("\"project\"", &format!("\"{text}\""));
        assert_eq!(verdict::parse(&json).unwrap().generality, want);
    }
}

#[test]
fn rejects_missing_extra_and_mistyped_fields() {
    let cases = [
        r#"{"repeatable":true,"user_correction":true,"root_cause":"x","proposed_fix_kind":"lint"}"#,
        r#"{"repeatable":true,"user_correction":true,"root_cause":"x","proposed_fix_kind":"lint","generality":"general","extra":1}"#,
        r#"{"repeatable":"yes","user_correction":true,"root_cause":"x","proposed_fix_kind":"lint","generality":"general"}"#,
        r#"{"repeatable":true,"user_correction":true,"root_cause":"x","proposed_fix_kind":"unknown","generality":"general"}"#,
        r#"{"repeatable":true,"user_correction":true,"root_cause":"x","proposed_fix_kind":"lint","generality":"global"}"#,
        r#"{"repeatable":true,"user_correction":true,"root_cause":"   ","proposed_fix_kind":"lint","generality":"general"}"#,
    ];
    for json in cases {
        assert!(verdict::parse(json).is_err(), "accepted: {json}");
    }
}

#[test]
fn rejects_text_that_is_not_one_json_object() {
    assert!(verdict::parse("").is_err());
    assert!(verdict::parse("Sure! Here is my answer").is_err());
    assert!(verdict::parse(&format!("[{VALID}]")).is_err());
}

#[test]
fn schema_lists_the_same_enum_values_the_parser_accepts() {
    let schema: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
    let props = &schema["properties"];
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        required,
        [
            "repeatable",
            "user_correction",
            "root_cause",
            "proposed_fix_kind",
            "generality"
        ]
    );
    assert_eq!(schema["additionalProperties"], false);
    for (field, values) in [
        ("proposed_fix_kind", &props["proposed_fix_kind"]["enum"]),
        ("generality", &props["generality"]["enum"]),
    ] {
        for value in values.as_array().unwrap() {
            let text = value.as_str().unwrap();
            let json = if field == "generality" {
                VALID.replace("\"project\"", &format!("\"{text}\""))
            } else {
                VALID.replace("agent-workflow", text)
            };
            assert!(verdict::parse(&json).is_ok(), "{field}={text} rejected");
        }
    }
}

#[test]
fn bound_truncates_at_a_char_boundary_and_says_so() {
    assert_eq!(bound("short", 10), "short");
    let out = bound("ééééé", 4);
    assert!(out.starts_with("éé"));
    assert!(out.ends_with("[truncated]"));
    assert_eq!(bound("ééééé", 4), out, "truncation must be deterministic");
}

#[test]
fn prompt_is_capped_and_lists_only_the_first_occurrences() {
    let occurrences: Vec<Occurrence> = (0..50)
        .map(|i| Occurrence {
            host: "claude".into(),
            ts: i,
            signals: vec!["tool-failure".into()],
            summary: "x".repeat(5_000),
        })
        .collect();
    let input = PromptInput {
        class: "failure",
        tool: Some("Bash".into()),
        pattern: "cargo test --test <n>".into(),
        sessions: 50,
        occurrences,
    };
    let prompt = build_prompt(&input);
    assert!(prompt.len() <= oncebitten::triage::prompt::MAX_PROMPT_BYTES + 64);
    assert!(prompt.contains("[truncated]"));
    assert_eq!(prompt, build_prompt(&input), "prompt must be deterministic");
    assert!(prompt.contains("sessions: 50"));
}
#[test]
fn mistake_class_is_an_optional_slug() {
    let with = VALID.replacen('{', r#"{"mistake_class":"unwrap-in-library","#, 1);
    assert_eq!(
        verdict::parse(&with).unwrap().mistake_class.as_deref(),
        Some("unwrap-in-library")
    );
    for absent in [
        VALID.replacen('{', r#"{"mistake_class":null,"#, 1),
        VALID.replacen('{', r#"{"mistake_class":"","#, 1),
    ] {
        assert_eq!(verdict::parse(&absent).unwrap().mistake_class, None);
    }
    let long = "a".repeat(65);
    for bad in ["Unwrap", "unwrap use", "-unwrap", long.as_str()] {
        let text = VALID.replacen('{', &format!(r#"{{"mistake_class":"{bad}","#), 1);
        assert!(verdict::parse(&text).is_err(), "accepted {bad}");
    }
}

#[test]
fn schema_declares_mistake_class_nullable_and_not_required() {
    let schema: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
    assert_eq!(
        schema["properties"]["mistake_class"]["type"],
        serde_json::json!(["string", "null"])
    );
    let required = schema["required"].as_array().unwrap();
    assert!(!required.iter().any(|key| key == "mistake_class"));
}
