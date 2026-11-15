use oncebitten::signal::{
    derive, derive_with, matched_phrases, normalize_command, Signal, KIND_PROMPT, KIND_TOOL_ERROR,
    KIND_TOOL_OK, REPEAT_FAILURE, REVERT, TEST_FAIL_THEN_PASS, TOOL_FAILURE, USER_CORRECTION,
};
use oncebitten::store::{NewEvent, StoredEvent};

fn tool(kind: &str, tool: &str, summary: &str, ts: i64) -> NewEvent {
    NewEvent {
        ts,
        host: "claude".into(),
        session_id: "s1".into(),
        repo: None,
        kind: kind.into(),
        tool: Some(tool.into()),
        summary: summary.into(),
    }
}

fn bash(summary: &str, ok: bool, ts: i64) -> NewEvent {
    let kind = if ok { KIND_TOOL_OK } else { KIND_TOOL_ERROR };
    tool(kind, "Bash", summary, ts)
}

fn prompt(text: &str, ts: i64) -> NewEvent {
    NewEvent {
        ts,
        host: "claude".into(),
        session_id: "s1".into(),
        repo: None,
        kind: KIND_PROMPT.into(),
        tool: None,
        summary: text.into(),
    }
}

fn past(events: Vec<NewEvent>) -> Vec<StoredEvent> {
    events
        .into_iter()
        .enumerate()
        .map(|(i, e)| StoredEvent {
            id: i as i64 + 1,
            ts: e.ts,
            host: e.host,
            session_id: e.session_id,
            repo: e.repo,
            kind: e.kind,
            tool: e.tool,
            summary: e.summary,
            git_head: None,
        })
        .collect()
}

fn kinds(signals: &[Signal]) -> Vec<&'static str> {
    signals.iter().map(|s| s.kind).collect()
}

#[test]
fn normalize_collapses_whitespace() {
    assert_eq!(
        normalize_command("cargo   test \t --lib"),
        "cargo test --lib"
    );
}

#[test]
fn normalize_replaces_digits_paths_and_hex() {
    assert_eq!(
        normalize_command("pytest tests/test_a.py::case_12"),
        "pytest <path>"
    );
    assert_eq!(normalize_command("git show 1a2b3c4d"), "git show <hex>");
    assert_eq!(normalize_command("curl 0xdeadbeef"), "curl <hex>");
    assert_eq!(normalize_command("sleep 30 retry 4"), "sleep <n> retry <n>");
}

#[test]
fn failed_tool_run_is_a_tool_failure() {
    let signals = derive(&bash("cargo build", false, 2), &[]);
    assert_eq!(kinds(&signals), vec![TOOL_FAILURE]);
}

#[test]
fn passing_tool_run_is_not_a_failure() {
    assert!(derive(&bash("cargo build", true, 2), &[]).is_empty());
}

#[test]
fn second_failure_with_same_normalized_command_repeats() {
    let history = past(vec![bash("cat /tmp/a1.txt", false, 1)]);
    let signals = derive(&bash("cat /tmp/b2.txt", false, 2), &history);
    assert_eq!(kinds(&signals), vec![TOOL_FAILURE, REPEAT_FAILURE]);
}

#[test]
fn different_failing_command_is_not_a_repeat() {
    let history = past(vec![bash("cat a.txt", false, 1)]);
    let signals = derive(&bash("cat b.txt", false, 2), &history);
    assert_eq!(kinds(&signals), vec![TOOL_FAILURE]);
}

#[test]
fn failure_with_same_command_from_another_tool_is_not_a_repeat() {
    let history = past(vec![tool(KIND_TOOL_ERROR, "Read", "cat a.txt", 1)]);
    let signals = derive(&bash("cat a.txt", false, 2), &history);
    assert_eq!(kinds(&signals), vec![TOOL_FAILURE]);
}

#[test]
fn test_pass_after_test_failure_is_fail_then_pass() {
    let history = past(vec![bash("cargo test", false, 1)]);
    let signals = derive(&bash("cargo test --lib", true, 2), &history);
    assert_eq!(kinds(&signals), vec![TEST_FAIL_THEN_PASS]);
}

#[test]
fn test_pass_after_a_passing_run_is_not_fail_then_pass() {
    let history = past(vec![bash("pytest", false, 1), bash("pytest", true, 2)]);
    assert!(derive(&bash("pytest", true, 3), &history).is_empty());
}

#[test]
fn test_pass_without_prior_failure_is_not_fail_then_pass() {
    assert!(derive(&bash("npm test", true, 1), &[]).is_empty());
}

#[test]
fn non_test_pass_after_failure_is_not_fail_then_pass() {
    let history = past(vec![bash("cargo test", false, 1)]);
    assert!(derive(&bash("ls", true, 2), &history).is_empty());
}

#[test]
fn every_test_runner_is_recognised() {
    for runner in [
        "cargo test",
        "pytest",
        "npm test",
        "vitest",
        "jest",
        "go test ./...",
    ] {
        let history = past(vec![bash(runner, false, 1)]);
        let signals = derive(&bash(runner, true, 2), &history);
        assert_eq!(
            kinds(&signals),
            vec![TEST_FAIL_THEN_PASS],
            "runner: {runner}"
        );
    }
}

#[test]
fn correction_lexicon_hits_are_user_corrections() {
    let hits = [
        "No, use the other API",
        "don't touch the lockfile",
        "Stop editing that file",
        "that's wrong",
        "I told you to use tabs",
        "never commit secrets",
        "always run fmt first",
        "that's not what I asked",
        "do it instead",
        "run it again",
    ];
    for text in hits {
        let signals = derive(&prompt(text, 1), &[]);
        assert_eq!(kinds(&signals), vec![USER_CORRECTION], "prompt: {text}");
    }
}

#[test]
fn ordinary_prompts_are_not_corrections() {
    let misses = [
        "Please add a test for the parser",
        "Summarise the README",
        "Know what I mean?",
        "Play the piano, then rest",
        "Read the against-list file",
        "Stopwatch the build",
    ];
    for text in misses {
        assert!(derive(&prompt(text, 1), &[]).is_empty(), "prompt: {text}");
    }
}

#[test]
fn git_revert_is_a_revert() {
    let signals = derive(&bash("git revert HEAD", true, 2), &[]);
    assert_eq!(kinds(&signals), vec![REVERT]);
}

#[test]
fn checkout_of_an_edited_file_is_a_revert() {
    let history = past(vec![tool(KIND_TOOL_OK, "Edit", "src/a.rs", 1)]);
    let signals = derive(&bash("git checkout -- src/a.rs", true, 2), &history);
    assert_eq!(kinds(&signals), vec![REVERT]);
}

#[test]
fn restore_of_an_edited_file_is_a_revert() {
    let history = past(vec![tool(KIND_TOOL_OK, "Write", "src/a.rs", 1)]);
    let signals = derive(&bash("git restore src/a.rs", true, 2), &history);
    assert_eq!(kinds(&signals), vec![REVERT]);
}

#[test]
fn checkout_of_an_unedited_file_or_a_branch_is_not_a_revert() {
    let history = past(vec![tool(KIND_TOOL_OK, "Edit", "src/a.rs", 1)]);
    assert!(derive(&bash("git checkout -- src/b.rs", true, 2), &history).is_empty());
    assert!(derive(&bash("git checkout main", true, 3), &history).is_empty());
}

#[test]
fn broad_lexicon_hits_are_user_corrections() {
    let hits = [
        "I already told you, use tabs",
        "Why did you rename that?",
        "You forgot the changelog",
        "undo that last change",
        "that broke the build",
        "it still doesn't work",
        "from now on use snake case",
        "please don't rewrite the parser",
        "make sure the tests pass",
    ];
    for text in hits {
        let signals = derive(&prompt(text, 1), &[]);
        assert_eq!(kinds(&signals), vec![USER_CORRECTION], "prompt: {text}");
    }
}

#[test]
fn correction_detail_lists_matched_phrases_not_the_prompt() {
    let signals = derive(&prompt("No, that's wrong and don't do it", 1), &[]);
    assert_eq!(signals.len(), 1);
    assert_eq!(
        signals[0].detail,
        "no,, that's wrong, wrong, don't, don't do"
    );
}

#[test]
fn matched_phrases_follow_the_given_lexicon() {
    let lexicon = ["tabs", "NO,"];
    let found = matched_phrases("No, use tabs. Tabs!", &lexicon);
    assert_eq!(found, vec!["tabs".to_string(), "no,".to_string()]);
}

#[test]
fn derive_with_uses_a_custom_lexicon_only() {
    let lexicon = ["spaces"];
    assert!(derive_with(&prompt("no, use tabs", 1), &[], &lexicon).is_empty());
    let signals = derive_with(&prompt("use spaces", 1), &[], &lexicon);
    assert_eq!(kinds(&signals), vec![USER_CORRECTION]);
}
