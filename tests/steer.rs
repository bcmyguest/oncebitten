use std::fs;
use std::path::PathBuf;

use oncebitten::replay::Verdict;
use oncebitten::rule::{load_rule, Action, Check, Mode, Rule, Steer, Trigger};
use oncebitten::steer::{self, Matcher, SteerEvent};
use tempfile::TempDir;

const HEAD: &str = "id = \"no-sudo\"\ndescription = \"Never run sudo.\"\nmode = \"steer\"\n\
status = \"active\"\nengine = \"steer\"\n";
const SUDO_TABLE: &str = "[steer]\naction = \"deny\"\nmessage = \"Ask the user to run sudo.\"\n\
[steer.trigger]\ntool = \"{bash,shell}\"\ncommand = '^\\s*sudo\\b'\n";
const BAD_SUDO: &str = r#"{"event": "tool", "tool": "Bash", "command": "sudo apt install jq"}"#;
const FIXED_SUDO: &str = r#"{"event": "tool", "tool": "Bash", "command": "apt list jq"}"#;

fn rule_dir(toml: &str, bad: &str, fixed: &str) -> (TempDir, PathBuf) {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("no-sudo");
    fs::create_dir_all(dir.join("fixtures")).unwrap();
    fs::write(dir.join("rule.toml"), toml).unwrap();
    fs::write(dir.join("fixtures/bad.json"), bad).unwrap();
    fs::write(dir.join("fixtures/fixed.json"), fixed).unwrap();
    (root, dir)
}

fn load(toml: &str) -> Rule {
    let (_root, dir) = rule_dir(toml, BAD_SUDO, FIXED_SUDO);
    load_rule(&dir).unwrap_or_else(|error| panic!("{error:#}"))
}

fn error_of(toml: &str) -> String {
    let (_root, dir) = rule_dir(toml, BAD_SUDO, FIXED_SUDO);
    format!("{:#}", load_rule(&dir).unwrap_err())
}

fn steer_of(rule: &Rule) -> &Steer {
    match &rule.check {
        Check::Steer(steer) => steer,
        other => panic!("expected a steer check, got {other:?}"),
    }
}

fn event(json: &str) -> SteerEvent {
    serde_json::from_str(json).unwrap()
}

fn matches(steer: &Steer, json: &str) -> bool {
    Matcher::new(steer).unwrap().matches(&event(json))
}

fn replay(toml: &str, bad: &str, fixed: &str) -> Verdict {
    let (_root, dir) = rule_dir(toml, bad, fixed);
    let rule = load_rule(&dir).unwrap();
    steer::replay(&dir, steer_of(&rule))
}

#[test]
fn steer_rule_loads_its_tool_trigger_action_and_message() {
    let rule = load(&format!("{HEAD}{SUDO_TABLE}"));
    assert_eq!(rule.mode, Mode::Steer);
    assert_eq!(
        steer_of(&rule),
        &Steer {
            trigger: Trigger::Tool {
                name: "{bash,shell}".to_owned(),
                command: Some(r"^\s*sudo\b".to_owned()),
            },
            action: Action::Deny,
            message: "Ask the user to run sudo.".to_owned(),
        }
    );
}

#[test]
fn mode_defaults_to_block() {
    let toml = "id = \"no-sudo\"\ndescription = \"x\"\nstatus = \"active\"\nengine = \"diff\"\n\
when_changed = [\"a\"]\nrequire_changed = [\"b\"]\n";
    assert_eq!(load(toml).mode, Mode::Block);
}

#[test]
fn action_defaults_to_inform() {
    let toml = format!("{HEAD}[steer]\nmessage = \"m\"\n[steer.trigger]\nprompt = \"deploy\"\n");
    assert_eq!(steer_of(&load(&toml)).action, Action::Inform);
}

#[test]
fn path_and_prompt_triggers_load() {
    let path = load(&format!(
        "{HEAD}[steer]\nmessage = \"m\"\n[steer.trigger]\npaths = [\"gen/**\"]\n"
    ));
    assert_eq!(
        steer_of(&path).trigger,
        Trigger::Path {
            globs: vec!["gen/**".to_owned()]
        }
    );
    let prompt = load(&format!(
        "{HEAD}[steer]\nmessage = \"m\"\n[steer.trigger]\nprompt = \"(?i)deploy\"\n"
    ));
    assert_eq!(
        steer_of(&prompt).trigger,
        Trigger::Prompt {
            pattern: "(?i)deploy".to_owned()
        }
    );
}

#[test]
fn deny_needs_a_tool_trigger() {
    for trigger in ["paths = [\"gen/**\"]", "prompt = \"x\""] {
        let message = error_of(&format!(
            "{HEAD}[steer]\naction = \"deny\"\nmessage = \"m\"\n[steer.trigger]\n{trigger}\n"
        ));
        assert!(message.contains("needs a tool trigger"), "{message}");
    }
}

#[test]
fn trigger_needs_exactly_one_kind() {
    for trigger in ["", "tool = \"bash\"\nprompt = \"x\"\n", "paths = []\n"] {
        let message = error_of(&format!(
            "{HEAD}[steer]\nmessage = \"m\"\n[steer.trigger]\n{trigger}"
        ));
        assert!(message.contains("[steer.trigger]"), "{message}");
    }
    let message = error_of(&format!(
        "{HEAD}[steer]\nmessage = \"m\"\n[steer.trigger]\ncommand = \"x\"\n"
    ));
    assert!(message.contains("command needs tool"), "{message}");
}

#[test]
fn bad_regex_and_glob_are_rejected_at_load() {
    let message = error_of(&format!(
        "{HEAD}[steer]\nmessage = \"m\"\n[steer.trigger]\nprompt = \"(\"\n"
    ));
    assert!(message.contains("regex"), "{message}");
    let message = error_of(&format!(
        "{HEAD}[steer]\nmessage = \"m\"\n[steer.trigger]\npaths = [\"a[\"]\n"
    ));
    assert!(message.contains("glob"), "{message}");
}

#[test]
fn steer_engine_needs_steer_mode_and_table() {
    let block = HEAD.replace("mode = \"steer\"", "mode = \"block\"");
    let message = error_of(&format!("{block}{SUDO_TABLE}"));
    assert!(message.contains("needs mode = \"steer\""), "{message}");
    let message = error_of(HEAD);
    assert!(message.contains("needs a [steer] table"), "{message}");
}

#[test]
fn steer_table_is_only_for_the_steer_engine() {
    let toml = format!(
        "id = \"no-sudo\"\ndescription = \"x\"\nstatus = \"active\"\nengine = \"diff\"\n\
when_changed = [\"a\"]\nrequire_changed = [\"b\"]\n{SUDO_TABLE}"
    );
    assert!(error_of(&toml).contains("only for engine = \"steer\""));
}

#[test]
fn empty_message_is_rejected() {
    let message = error_of(&format!(
        "{HEAD}[steer]\nmessage = \" \"\n[steer.trigger]\nprompt = \"x\"\n"
    ));
    assert!(message.contains("message is empty"), "{message}");
}

#[test]
fn steer_fixtures_must_be_json() {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("no-sudo");
    fs::create_dir_all(dir.join("fixtures")).unwrap();
    fs::write(dir.join("rule.toml"), format!("{HEAD}{SUDO_TABLE}")).unwrap();
    fs::write(dir.join("fixtures/bad.txt"), BAD_SUDO).unwrap();
    fs::write(dir.join("fixtures/fixed.json"), FIXED_SUDO).unwrap();
    let message = format!("{:#}", load_rule(&dir).unwrap_err());
    assert!(message.contains("fixtures/bad.json"), "{message}");
}

#[test]
fn tool_trigger_matches_name_case_insensitively_and_command_by_regex() {
    let rule = load(&format!("{HEAD}{SUDO_TABLE}"));
    let steer = steer_of(&rule);
    assert!(matches(steer, BAD_SUDO));
    assert!(matches(
        steer,
        r#"{"event": "tool", "tool": "shell", "command": "  sudo -i"}"#
    ));
    assert!(!matches(steer, FIXED_SUDO));
    assert!(!matches(
        steer,
        r#"{"event": "tool", "tool": "Bash", "command": "echo sudo"}"#
    ));
    assert!(!matches(
        steer,
        r#"{"event": "tool", "tool": "Bash", "command": "sudoku"}"#
    ));
    assert!(!matches(
        steer,
        r#"{"event": "tool", "tool": "Write", "paths": ["sudo"]}"#
    ));
    assert!(!matches(steer, r#"{"event": "prompt", "prompt": "sudo"}"#));
}

#[test]
fn tool_trigger_without_command_matches_any_call_of_the_tool() {
    let rule = load(&format!(
        "{HEAD}[steer]\nmessage = \"m\"\n[steer.trigger]\ntool = \"WebFetch\"\n"
    ));
    assert!(matches(
        steer_of(&rule),
        r#"{"event": "tool", "tool": "WebFetch"}"#
    ));
    assert!(!matches(
        steer_of(&rule),
        r#"{"event": "tool", "tool": "Bash", "command": "curl x"}"#
    ));
}

#[test]
fn path_trigger_matches_edited_paths_by_glob() {
    let rule = load(&format!(
        "{HEAD}[steer]\nmessage = \"m\"\n[steer.trigger]\npaths = [\"src/gen/**\"]\n"
    ));
    let steer = steer_of(&rule);
    assert!(matches(
        steer,
        r#"{"event": "tool", "tool": "Edit", "paths": ["src/gen/api.rs"]}"#
    ));
    assert!(!matches(
        steer,
        r#"{"event": "tool", "tool": "Edit", "paths": ["src/api.rs"]}"#
    ));
    assert!(!matches(
        steer,
        r#"{"event": "prompt", "prompt": "src/gen/x"}"#
    ));
}

#[test]
fn prompt_trigger_matches_prompt_text() {
    let rule = load(&format!(
        "{HEAD}[steer]\nmessage = \"m\"\n[steer.trigger]\nprompt = \"(?i)\\\\bdeploy\\\\b\"\n"
    ));
    let steer = steer_of(&rule);
    assert!(matches(
        steer,
        r#"{"event": "prompt", "prompt": "Deploy it now"}"#
    ));
    assert!(!matches(
        steer,
        r#"{"event": "prompt", "prompt": "redeployment"}"#
    ));
}

#[test]
fn replay_proves_a_rule_whose_bad_fixture_triggers_and_fixed_does_not() {
    let toml = format!("{HEAD}{SUDO_TABLE}");
    assert_eq!(replay(&toml, BAD_SUDO, FIXED_SUDO), Verdict::Proven);
}

#[test]
fn replay_rejects_when_bad_is_silent_or_fixed_triggers() {
    let toml = format!("{HEAD}{SUDO_TABLE}");
    match replay(&toml, FIXED_SUDO, FIXED_SUDO) {
        Verdict::Rejected(reason) => assert!(reason.contains("did not trigger"), "{reason}"),
        other => panic!("{other:?}"),
    }
    match replay(&toml, BAD_SUDO, BAD_SUDO) {
        Verdict::Rejected(reason) => assert!(reason.contains("fixed.json triggered"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn replay_checks_the_corpus() {
    let toml = format!("{HEAD}{SUDO_TABLE}");
    let (_root, dir) = rule_dir(&toml, BAD_SUDO, FIXED_SUDO);
    fs::create_dir_all(dir.join("corpus")).unwrap();
    fs::write(dir.join("corpus/also-sudo.json"), BAD_SUDO).unwrap();
    let rule = load_rule(&dir).unwrap();
    match steer::replay(&dir, steer_of(&rule)) {
        Verdict::Rejected(reason) => assert!(reason.contains("also-sudo.json"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn replay_could_not_run_on_a_fixture_that_is_not_an_event() {
    let toml = format!("{HEAD}{SUDO_TABLE}");
    match replay(&toml, "{\"event\": \"other\"}", FIXED_SUDO) {
        Verdict::CouldNotRun(reason) => assert!(reason.contains("bad.json"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn events_round_trip_through_json() {
    let tool = SteerEvent::Tool {
        tool: "Edit".to_owned(),
        command: None,
        paths: vec!["a/b.rs".to_owned()],
    };
    let text = serde_json::to_string(&tool).unwrap();
    assert_eq!(text, r#"{"event":"tool","tool":"Edit","paths":["a/b.rs"]}"#);
    assert_eq!(event(&text), tool);
}

#[test]
fn shipped_no_sudo_example_is_proposed_and_proves() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/no-sudo");
    let rule = load_rule(&dir).unwrap();
    assert_eq!(rule.status, oncebitten::rule::Status::Proposed);
    assert_eq!(steer_of(&rule).action, Action::Deny);
    assert_eq!(steer::replay(&dir, steer_of(&rule)), Verdict::Proven);
}
